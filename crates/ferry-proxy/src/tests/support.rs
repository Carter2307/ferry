//! Test harness: a proxy on ephemeral ports, scriptable upstream servers and
//! small clients.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Once};
use std::time::Duration;

use bytes::Bytes;
use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Result};
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::body::{self, ProxyBody};
use crate::{ProxyConfig, RouteTable, server};

pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Await with a deadline so a hung proxy fails the test instead of blocking it.
pub(crate) async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(TIMEOUT, fut).await {
        Ok(out) => out,
        Err(_) => panic!("timed out: {what}"),
    }
}

// ---------------------------------------------------------------- proxy ----

pub(crate) struct TestProxy {
    pub http: SocketAddr,
    pub https: Option<SocketAddr>,
    pub routes: RouteTable,
    pub shutdown: CancellationToken,
    task: Option<JoinHandle<Result<()>>>,
}

pub(crate) struct Options {
    pub tls: Option<Arc<rustls::ServerConfig>>,
    pub hooks: Option<Arc<dyn TlsHooks>>,
    pub redirect_https: bool,
    pub drain: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options { tls: None, hooks: None, redirect_https: false, drain: server::DRAIN_TIMEOUT }
    }
}

impl TestProxy {
    pub(crate) async fn start() -> Self {
        Self::start_with(Options::default()).await
    }

    pub(crate) async fn start_with(opts: Options) -> Self {
        let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = http_listener.local_addr().unwrap();
        let (https_listener, https) = if opts.tls.is_some() {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            (Some(l), Some(addr))
        } else {
            (None, None)
        };
        let config = ProxyConfig {
            http_addr: http,
            https_addr: https,
            tls: opts.tls,
            tls_hooks: opts.hooks,
            redirect_https: opts.redirect_https,
        };
        let routes = RouteTable::new();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let routes = routes.clone();
            let shutdown = shutdown.clone();
            async move { server::serve_on(&config, http_listener, https_listener, routes, shutdown, opts.drain).await }
        });
        TestProxy { http, https, routes, shutdown, task: Some(task) }
    }

    pub(crate) fn route(&self, service: &str, hosts: &[&str], upstreams: &[SocketAddr]) {
        let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
        self.routes.set_service_routes(service, &hosts, upstreams.to_vec());
    }

    /// Cancel and wait for `serve_on` to return; yields how long it took.
    pub(crate) async fn stop(&mut self) -> Duration {
        let started = std::time::Instant::now();
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            within("proxy shutdown", task).await.unwrap().unwrap();
        }
        started.elapsed()
    }

    pub(crate) fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.http.port())
    }
}

impl Drop for TestProxy {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// ------------------------------------------------------------ upstreams ----

/// An HTTP/1.1 server on 127.0.0.1; aborted on drop.
pub(crate) struct Upstream {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn spawn_upstream<F, Fut>(handler: F) -> Upstream
where
    F: Fn(Request<Incoming>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Response<ProxyBody>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
        let mut conns = tokio::task::JoinSet::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let handler = handler.clone();
            conns.spawn(async move {
                let svc = service_fn(move |req| {
                    let fut = handler(req);
                    async move { Ok::<_, Infallible>(fut.await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .with_upgrades()
                    .await;
            });
        }
    });
    Upstream { addr, task }
}

/// FNV-1a, to compare large bodies without keeping them.
pub(crate) fn fnv(hash: u64, data: &[u8]) -> u64 {
    data.iter().fold(hash, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}
pub(crate) const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// Upstream answering with a plain-text dump of what it received:
/// `upstream=<name>`, `method=`, `path=`, `version=`, `header:<name>=<value>`
/// lines, `body-bytes=` and `body-fnv=`. `x-upstream: <name>` is also set.
/// Path `/hop` adds hop-by-hop response headers that must not reach clients.
pub(crate) async fn echo_upstream(name: &'static str) -> Upstream {
    spawn_upstream(move |req: Request<Incoming>| async move {
        let mut lines = vec![
            format!("upstream={name}"),
            format!("method={}", req.method()),
            format!("path={}", req.uri().path_and_query().map_or("", |p| p.as_str())),
            format!("version={:?}", req.version()),
        ];
        for (k, v) in req.headers() {
            lines.push(format!("header:{}={}", k, v.to_str().unwrap_or("<binary>")));
        }
        let hop = req.uri().path() == "/hop";
        let mut body = req.into_body();
        let (mut n, mut hash) = (0u64, FNV_START);
        while let Some(frame) = body.frame().await {
            let Ok(frame) = frame else { break };
            if let Ok(data) = frame.into_data() {
                n += data.len() as u64;
                hash = fnv(hash, &data);
            }
        }
        lines.push(format!("body-bytes={n}"));
        lines.push(format!("body-fnv={hash:x}"));
        let mut resp = Response::builder().header("x-upstream", name).header("content-type", "text/plain");
        if hop {
            resp = resp
                .header("connection", "x-up-hop")
                .header("x-up-hop", "secret")
                .header("keep-alive", "timeout=5")
                .header("proxy-authenticate", "Basic")
                .header("x-end-to-end", "kept");
        }
        resp.body(body::full(lines.join("\n") + "\n")).unwrap()
    })
    .await
}

/// Parsed dump from [`echo_upstream`].
#[derive(Debug)]
pub(crate) struct Echo {
    pub upstream: String,
    pub method: String,
    pub path: String,
    pub version: String,
    pub headers: HashMap<String, Vec<String>>,
    pub body_bytes: u64,
    pub body_fnv: String,
}

impl Echo {
    pub(crate) fn parse(text: &str) -> Echo {
        let mut echo = Echo {
            upstream: String::new(),
            method: String::new(),
            path: String::new(),
            version: String::new(),
            headers: HashMap::new(),
            body_bytes: 0,
            body_fnv: String::new(),
        };
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("header:") {
                let (k, v) = rest.split_once('=').unwrap();
                echo.headers.entry(k.to_string()).or_default().push(v.to_string());
            } else if let Some((k, v)) = line.split_once('=') {
                match k {
                    "upstream" => echo.upstream = v.into(),
                    "method" => echo.method = v.into(),
                    "path" => echo.path = v.into(),
                    "version" => echo.version = v.into(),
                    "body-bytes" => echo.body_bytes = v.parse().unwrap(),
                    "body-fnv" => echo.body_fnv = v.into(),
                    _ => {}
                }
            }
        }
        echo
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.first()).map(String::as_str)
    }
}

/// An address nothing listens on (connections are refused).
pub(crate) async fn dead_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap()
}

// -------------------------------------------------------------- clients ----

pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve("app.test", "127.0.0.1:0".parse().unwrap())
        .timeout(TIMEOUT)
        .build()
        .unwrap()
}

/// reqwest client resolving each of `hosts` to 127.0.0.1.
pub(crate) fn client_for(hosts: &[&str]) -> reqwest::ClientBuilder {
    let mut b = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(TIMEOUT);
    for h in hosts {
        b = b.resolve(h, "127.0.0.1:0".parse().unwrap());
    }
    b
}

/// A response with its body collected.
pub(crate) struct Collected {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
    pub body: String,
}

/// One HTTP/1.1 request on a fresh connection with exactly the given headers
/// (hyper's client adds nothing but `content-length`/`transfer-encoding`).
pub(crate) async fn h1_send(addr: SocketAddr, req: Request<Full<Bytes>>) -> Collected {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let resp = within("h1 request", sender.send_request(req)).await.unwrap();
    collect(resp).await
}

/// One HTTP/2 prior-knowledge (h2c) request.
pub(crate) async fn h2c_send(addr: SocketAddr, req: Request<Full<Bytes>>) -> Collected {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let resp = within("h2c request", sender.send_request(req)).await.unwrap();
    collect(resp).await
}

pub(crate) async fn collect(resp: Response<Incoming>) -> Collected {
    let (parts, body) = resp.into_parts();
    let body = within("response body", body.collect()).await.unwrap().to_bytes();
    Collected { status: parts.status, headers: parts.headers, body: String::from_utf8_lossy(&body).into_owned() }
}

/// Write raw bytes and read until the server closes the connection.
pub(crate) async fn raw_exchange(addr: SocketAddr, request: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    within("raw response", stream.read_to_end(&mut out)).await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

// ------------------------------------------------------------ TLS hooks ----

#[derive(Default)]
pub(crate) struct MockHooks {
    pub challenges: HashMap<String, String>,
    pub certificates: HashSet<String>,
}

impl TlsHooks for MockHooks {
    fn http01_response(&self, token: &str) -> Option<String> {
        self.challenges.get(token).cloned()
    }

    fn has_certificate(&self, host: &str) -> bool {
        self.certificates.contains(host)
    }
}

/// Self-signed certificate for `hosts`: (server config with ALPN h2 +
/// http/1.1, DER certificate for client trust stores).
pub(crate) fn self_signed(hosts: &[&str]) -> (Arc<rustls::ServerConfig>, rustls::pki_types::CertificateDer<'static>) {
    install_crypto();
    let names: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
    let certified = rcgen::generate_simple_self_signed(names).unwrap();
    let cert = certified.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let mut config =
        rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert.clone()], key).unwrap();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    (Arc::new(config), cert)
}
