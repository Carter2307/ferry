//! Per-request logic: ACME challenges, host routing, HTTPS redirects,
//! forwarding (with retry), upgrades and error pages.

use std::convert::Infallible;
use std::error::Error as StdError;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::CancellationToken;
use ferry_core::domains::{PROBE_PATH, is_probe_token, probe_answer};
use ferry_core::tls::TlsHooks;
use http::header::{CONNECTION, HOST, HeaderValue};
use http::uri::{Authority, PathAndQuery, Scheme};
use http::{HeaderMap, Method, Request, Response, StatusCode, Uri, Version};
use hyper::body::{Body as _, Incoming};
use hyper::upgrade::OnUpgrade;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio_util::task::TaskTracker;

use crate::ProxyConfig;
use crate::body::{self, BodyTimedOut, ProxyBody};
use crate::headers::{self, Forwarded};
use crate::limits::ConnSlot;
use crate::pages::{self, ErrorKind};
use crate::routes::{Resolution, RouteTable, normalize_host};

const ACME_PREFIX: &str = "/.well-known/acme-challenge/";
/// Upstreams are containers on 127.0.0.1: anything slower than this to accept
/// a TCP connection is not coming back.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Idle upstream connections are dropped well before common app-server
/// keep-alive timeouts (Node/uvicorn 5 s, gunicorn 2 s) so the proxy rarely
/// reuses a connection the app is closing. Reconnecting on loopback is cheap.
const UPSTREAM_POOL_IDLE: Duration = Duration::from_secs(1);
const UPSTREAM_POOL_MAX_IDLE_PER_HOST: usize = 64;
/// Text of the `no_upstreams` page. The route table only knows that the
/// service has no instance to send traffic to — not why (never deployed,
/// last deploy failed, deploy in progress, instances restarting) — so the
/// page lists the likely reasons without claiming one.
pub(crate) const NO_UPSTREAMS_MESSAGE: &str = "This service has no running instances right now — it may not be deployed \
     yet, its last deploy may have failed, or a deploy is in progress.";

/// The client side of a connection.
#[derive(Clone)]
pub(crate) struct ConnInfo {
    pub remote: SocketAddr,
    pub tls: bool,
    /// The connection's slot in the connection limits; websocket tunnels
    /// hold it until they close.
    pub slot: Arc<ConnSlot>,
}

/// State shared by every connection of one `serve` call.
pub(crate) struct Proxy {
    routes: RouteTable,
    client: Client<HttpConnector, ProxyBody>,
    tls_hooks: Option<Arc<dyn TlsHooks>>,
    redirect_https: bool,
    /// Port of the running HTTPS listener (None → never redirect).
    https_port: Option<u16>,
    /// What domain verification probes are answered with (None → not answered).
    domain_probe_id: Option<String>,
    /// Longest wait for the next piece of a client's request body.
    request_body_timeout: Duration,
    /// Upgraded (websocket) tunnels are tracked with the connections...
    tasks: TaskTracker,
    /// ...and closed when shutdown starts.
    shutdown: CancellationToken,
}

impl Proxy {
    /// `https_port`: the port of the running HTTPS listener, if any.
    pub(crate) fn new(
        config: &ProxyConfig,
        routes: RouteTable,
        https_port: Option<u16>,
        tasks: TaskTracker,
        shutdown: CancellationToken,
    ) -> Self {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(UPSTREAM_CONNECT_TIMEOUT));
        connector.set_nodelay(true);
        connector.enforce_http(true);
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(UPSTREAM_POOL_IDLE)
            .pool_max_idle_per_host(UPSTREAM_POOL_MAX_IDLE_PER_HOST)
            .pool_timer(TokioTimer::new())
            .timer(TokioTimer::new())
            .build(connector);
        Proxy {
            routes,
            client,
            tls_hooks: config.tls_hooks.clone(),
            redirect_https: config.redirect_https,
            https_port,
            domain_probe_id: config.domain_probe_id.clone(),
            request_body_timeout: config.limits.request_body_timeout,
            tasks,
            shutdown,
        }
    }

    /// hyper service entry point. Never fails: every problem becomes a response.
    pub(crate) async fn handle(
        self: Arc<Self>,
        conn: ConnInfo,
        req: Request<Incoming>,
    ) -> Result<Response<ProxyBody>, Infallible> {
        let started = Instant::now();
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let mut log = RequestLog::default();
        let resp = self.dispatch(&conn, req, &mut log).await;
        tracing::debug!(
            method = %method,
            host = log.host.as_deref().unwrap_or("-"),
            path = %path,
            status = resp.status().as_u16(),
            upstream = %log.upstream.map_or_else(|| "-".to_string(), |a| a.to_string()),
            ms = started.elapsed().as_millis() as u64,
            client = %conn.remote,
            "proxy request"
        );
        Ok(resp)
    }

    async fn dispatch(&self, conn: &ConnInfo, req: Request<Incoming>, log: &mut RequestLog) -> Response<ProxyBody> {
        let head = req.method() == Method::HEAD;

        // 1. ACME HTTP-01 challenges (plain HTTP only).
        if !conn.tls
            && (req.method() == Method::GET || head)
            && let (Some(hooks), Some(token)) = (&self.tls_hooks, req.uri().path().strip_prefix(ACME_PREFIX))
        {
            return match hooks.http01_response(token) {
                Some(key_authorization) => pages::plain_answer(key_authorization, head),
                None => pages::error_page(ErrorKind::NotFound, "Unknown ACME challenge", head),
            };
        }

        // Domain verification (DESIGN.md §21): the server that asks wants
        // to know which server a name reaches, so every host is answered,
        // routed or not, before anything that depends on the host.
        if (req.method() == Method::GET || head)
            && let (Some(probe_id), Some(token)) = (&self.domain_probe_id, req.uri().path().strip_prefix(PROBE_PATH))
            && is_probe_token(token)
        {
            return pages::plain_answer(probe_answer(token, probe_id), head);
        }

        if req.method() == Method::CONNECT {
            return pages::error_page(ErrorKind::MethodNotAllowed, "CONNECT is not supported by this server", head);
        }

        // 2. Host.
        let Some((raw_host, host_from_uri)) = request_host(&req) else {
            return pages::error_page(ErrorKind::BadRequest, "Bad request: missing or invalid Host header", head);
        };
        let host = normalize_host(&raw_host);
        log.host = Some(host.clone());

        // 3. HTTP → HTTPS for hosts with a certificate.
        if !conn.tls
            && self.redirect_https
            && let (Some(port), Some(hooks)) = (self.https_port, &self.tls_hooks)
            && hooks.has_certificate(&host)
        {
            return match https_location(&host, port, req.uri()) {
                Some(location) => pages::redirect(location, head),
                None => pages::error_page(ErrorKind::BadRequest, "Bad request: invalid request target", head),
            };
        }

        // 4. Route.
        let (upstream, fallback) = match self.routes.resolve_with_fallback(&host) {
            (Resolution::Upstream(upstream), fallback) => (upstream, fallback),
            (Resolution::NotFound, _) => {
                let message = format!("No service is configured for {host}");
                return pages::error_page(ErrorKind::NotFound, &message, head);
            }
            (Resolution::Suspended, _) => {
                return pages::error_page(ErrorKind::Suspended, "This service is suspended", head);
            }
            (Resolution::NoUpstreams, _) => {
                return pages::error_page(ErrorKind::NoUpstreams, NO_UPSTREAMS_MESSAGE, head);
            }
        };

        // 5. Forward.
        self.forward(conn, req, (&raw_host, host_from_uri), upstream, fallback, log).await
    }

    async fn forward(
        &self,
        conn: &ConnInfo,
        mut req: Request<Incoming>,
        (raw_host, host_from_uri): (&str, bool),
        upstream: SocketAddr,
        fallback: Option<SocketAddr>,
        log: &mut RequestLog,
    ) -> Response<ProxyBody> {
        let head = req.method() == Method::HEAD;
        let upgrade = headers::requested_upgrade(req.version(), req.headers());
        // Must be taken before the request is torn apart (it lives in the extensions).
        let client_upgrade = upgrade.is_some().then(|| hyper::upgrade::on(&mut req));
        let http2 = req.version() == Version::HTTP_2;

        let (parts, incoming) = req.into_parts();
        let path_and_query = match parts.uri.path_and_query() {
            Some(pq) => pq.clone(),
            None => PathAndQuery::from_static("/"),
        };
        let mut upstream_headers = headers::upstream_request_headers(
            parts.headers,
            Forwarded {
                client_ip: conn.remote.ip(),
                tls: conn.tls,
                host: raw_host,
                replace_host: host_from_uri,
                http2,
                upgrade,
            },
        );

        if !incoming.is_end_stream() && incoming.size_hint().exact().is_none() {
            // A body of unknown length (HTTP/1 chunked, HTTP/2 without
            // content-length) must be sent chunked: left to itself, hyper's
            // client sends GET/HEAD bodies of unknown length as empty.
            headers::set_chunked(&mut upstream_headers);
        }

        // Only requests without a body can be replayed on another upstream.
        let replayable = (parts.method == Method::GET || parts.method == Method::HEAD)
            && incoming.is_end_stream()
            && client_upgrade.is_none();
        // A client that stops sending its body fails the upstream request
        // (→ 408 below) instead of holding both connections forever.
        let first_body =
            if replayable { body::empty() } else { body::with_idle_timeout(incoming, self.request_body_timeout) };
        let retry_headers = replayable.then(|| upstream_headers.clone());

        log.upstream = Some(upstream);
        let first = self.send(upstream, &parts.method, &path_and_query, upstream_headers, first_body).await;
        let result = match (first, retry_headers) {
            (Err(err), Some(headers)) => match retry_target(&err, upstream, fallback) {
                Some(target) => {
                    tracing::warn!(
                        host = raw_host,
                        upstream = %upstream,
                        retry = %target,
                        "proxy: upstream request failed ({}); retrying",
                        error_chain(&err)
                    );
                    log.upstream = Some(target);
                    self.send(target, &parts.method, &path_and_query, headers, body::empty()).await
                }
                None => Err(err),
            },
            (result, _) => result,
        };

        let resp = match result {
            Ok(resp) => resp,
            Err(err) if BodyTimedOut::caused(&err) => {
                // Nothing came back from the upstream yet, so the client can
                // still be told; the rest of its body is never read, so its
                // connection cannot be reused.
                tracing::debug!(
                    host = raw_host,
                    upstream = %upstream,
                    client = %conn.remote,
                    "proxy: request aborted: {}",
                    error_chain(&err)
                );
                let mut resp = pages::error_page(
                    ErrorKind::RequestTimeout,
                    "Request timeout: the request body stopped arriving",
                    head,
                );
                if !http2 {
                    resp.headers_mut().insert(CONNECTION, HeaderValue::from_static("close"));
                }
                return resp;
            }
            Err(err) => {
                tracing::warn!(
                    host = raw_host,
                    upstream = %log.upstream.unwrap_or(upstream),
                    "proxy: upstream request failed: {}",
                    error_chain(&err)
                );
                return pages::error_page(ErrorKind::BadGateway, "Bad gateway: the service is not responding", head);
            }
        };

        if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            return match client_upgrade {
                Some(client_upgrade) => self.tunnel(resp, client_upgrade, raw_host, conn.slot.clone()),
                None => {
                    tracing::warn!(host = raw_host, upstream = %upstream, "proxy: upstream switched protocols unasked");
                    pages::error_page(ErrorKind::BadGateway, "Bad gateway: the service sent an invalid response", head)
                }
            };
        }

        let (mut parts, incoming) = resp.into_parts();
        headers::client_response_headers(&mut parts.headers, false);
        Response::from_parts(parts, body::stream(incoming))
    }

    /// Send one request upstream as HTTP/1.1.
    async fn send(
        &self,
        upstream: SocketAddr,
        method: &Method,
        path_and_query: &PathAndQuery,
        headers: HeaderMap,
        body: ProxyBody,
    ) -> Result<Response<Incoming>, UpstreamError> {
        let uri = upstream_uri(upstream, path_and_query).map_err(UpstreamError::InvalidTarget)?;
        let mut req = Request::new(body);
        *req.method_mut() = method.clone();
        *req.uri_mut() = uri;
        *req.version_mut() = Version::HTTP_11;
        *req.headers_mut() = headers;
        self.client.request(req).await.map_err(UpstreamError::Client)
    }

    /// Answer `101` to the client and splice both upgraded connections. The
    /// tunnel keeps the client connection's `slot` while it is open.
    fn tunnel(
        &self,
        mut resp: Response<Incoming>,
        client_upgrade: OnUpgrade,
        host: &str,
        slot: Arc<ConnSlot>,
    ) -> Response<ProxyBody> {
        let upstream_upgrade = hyper::upgrade::on(&mut resp);
        let (mut parts, _) = resp.into_parts();
        headers::client_response_headers(&mut parts.headers, true);

        let shutdown = self.shutdown.clone();
        let host = host.to_string();
        self.tasks.spawn(async move {
            let _slot = slot;
            // The client side completes once hyper has written the 101 below.
            let upgraded = tokio::select! {
                res = async { tokio::try_join!(client_upgrade, upstream_upgrade) } => res,
                _ = shutdown.cancelled() => return,
            };
            let (client, upstream) = match upgraded {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::debug!(host = %host, "proxy: upgrade did not complete: {e}");
                    return;
                }
            };
            let mut client = TokioIo::new(client);
            let mut upstream = TokioIo::new(upstream);
            tokio::select! {
                res = tokio::io::copy_bidirectional(&mut client, &mut upstream) => match res {
                    Ok((up, down)) => tracing::debug!(host = %host, "proxy: tunnel closed ({up} B up, {down} B down)"),
                    Err(e) => tracing::debug!(host = %host, "proxy: tunnel closed: {e}"),
                },
                _ = shutdown.cancelled() => tracing::debug!(host = %host, "proxy: closing tunnel for shutdown"),
            }
        });

        Response::from_parts(parts, body::empty())
    }
}

#[derive(Debug, Default)]
struct RequestLog {
    host: Option<String>,
    upstream: Option<SocketAddr>,
}

#[derive(Debug)]
enum UpstreamError {
    InvalidTarget(http::Error),
    Client(hyper_util::client::legacy::Error),
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamError::InvalidTarget(e) => write!(f, "invalid request target: {e}"),
            UpstreamError::Client(e) => e.fmt(f),
        }
    }
}

impl StdError for UpstreamError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            UpstreamError::InvalidTarget(e) => Some(e),
            UpstreamError::Client(e) => Some(e),
        }
    }
}

/// Where to retry a replayable (bodyless GET/HEAD) request, if anywhere:
/// * connect failure → another upstream of the service, if it has one;
/// * connection closed before any response (a keep-alive connection the app
///   was closing, or a port forwarder whose container just went away) →
///   another upstream, else the same one on a fresh connection.
fn retry_target(err: &UpstreamError, upstream: SocketAddr, fallback: Option<SocketAddr>) -> Option<SocketAddr> {
    let UpstreamError::Client(err) = err else { return None };
    if err.is_connect() {
        return fallback;
    }
    let closed_early = err
        .source()
        .and_then(|e| e.downcast_ref::<hyper::Error>())
        .is_some_and(|e| e.is_incomplete_message() || e.is_canceled());
    if closed_early { fallback.or(Some(upstream)) } else { None }
}

/// The host the client asked for, and whether it came from the request URI.
///
/// The URI authority wins when present — HTTP/2 `:authority`, or an HTTP/1
/// absolute-form target, for which RFC 9112 §3.2.2 says `Host` is ignored —
/// otherwise the `Host` header. `None` when missing, duplicated or not a valid
/// `host[:port]`.
fn request_host(req: &Request<Incoming>) -> Option<(String, bool)> {
    let mut host_headers = req.headers().get_all(HOST).iter();
    let header = host_headers.next();
    if host_headers.next().is_some() {
        return None;
    }
    let (raw, from_uri) = match req.uri().authority() {
        Some(authority) => (authority.as_str(), true),
        None => (header?.to_str().ok()?.trim(), false),
    };
    let parsed: Authority = raw.parse().ok()?;
    if raw.contains('@') || parsed.host().is_empty() {
        return None;
    }
    Some((raw.to_string(), from_uri))
}

/// `https://host[:port]/path?query` for the redirect.
fn https_location(host: &str, https_port: u16, uri: &Uri) -> Option<HeaderValue> {
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let port = if https_port == 443 { String::new() } else { format!(":{https_port}") };
    let path = uri.path_and_query().map_or("/", PathAndQuery::as_str);
    let path = if path.starts_with('/') { path } else { "/" };
    HeaderValue::from_str(&format!("https://{host}{port}{path}")).ok()
}

fn upstream_uri(upstream: SocketAddr, path_and_query: &PathAndQuery) -> Result<Uri, http::Error> {
    Uri::builder().scheme(Scheme::HTTP).authority(upstream.to_string()).path_and_query(path_and_query.clone()).build()
}

/// "client error (Connect): tcp connect error: Connection refused" etc.
fn error_chain(err: &dyn StdError) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        let msg = e.to_string();
        if !out.contains(&msg) {
            out.push_str(": ");
            out.push_str(&msg);
        }
        source = e.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_redirect_locations() {
        let uri: Uri = "/a/b?c=1&d=%20".parse().unwrap();
        assert_eq!(https_location("app.example.com", 443, &uri).unwrap(), "https://app.example.com/a/b?c=1&d=%20");
        assert_eq!(
            https_location("app.example.com", 8443, &uri).unwrap(),
            "https://app.example.com:8443/a/b?c=1&d=%20"
        );
        assert_eq!(https_location("::1", 443, &"/".parse().unwrap()).unwrap(), "https://[::1]/");
        assert_eq!(https_location("a.test", 443, &"*".parse().unwrap()).unwrap(), "https://a.test/");
    }

    #[test]
    fn builds_upstream_uris() {
        let pq = PathAndQuery::from_static("/x/y?z=1");
        assert_eq!(upstream_uri("127.0.0.1:5000".parse().unwrap(), &pq).unwrap(), "http://127.0.0.1:5000/x/y?z=1");
        assert_eq!(upstream_uri("[::1]:5000".parse().unwrap(), &pq).unwrap(), "http://[::1]:5000/x/y?z=1");
    }
}
