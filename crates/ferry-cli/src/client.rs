//! Thin HTTP client for the Ferry API (`/api/v1`): bearer auth, JSON helpers,
//! friendly transport errors, streamed uploads and SSE log streams.

use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use ferry_core::dto::ApiErrorBody;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Method, RequestBuilder, Response, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::sse::{SseEvent, SseParser};

/// Default timeout for ordinary (non-streaming) requests. Some calls do real
/// work synchronously on the server (blueprint apply, deletes), hence generous.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A non-2xx answer from the API. Displays as `<message> (<code>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// Build from a status and the raw response body.
    pub fn from_body(status: u16, body: &[u8]) -> Self {
        if let Ok(b) = serde_json::from_slice::<ApiErrorBody>(body) {
            return ApiError { status, code: b.error.code, message: b.error.message };
        }
        let text = String::from_utf8_lossy(body);
        let text = text.trim();
        let reason = reqwest::StatusCode::from_u16(status).ok().and_then(|s| s.canonical_reason()).unwrap_or("error");
        let message = if text.is_empty() {
            format!("server returned {status} {reason}")
        } else {
            format!("server returned {status} {reason}: {}", crate::output::truncate(text, 200))
        };
        ApiError { status, code: format!("http_{status}"), message }
    }
}

/// True when `err` is (or wraps) an API 404.
pub fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<ApiError>().is_some_and(|a| a.status == 404))
}

/// True when `err` is a 404 for an API route the server doesn't have (an
/// older ferryd), as opposed to a missing resource.
pub fn is_missing_route(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|e| e.downcast_ref::<ApiError>())
        .any(|a| a.status == 404 && (a.message.starts_with("no API route") || a.code == "http_404"))
}

/// A decoded JSON response plus the raw value (printed as-is by `--json`).
#[derive(Debug, Clone)]
pub struct Json<T> {
    pub data: T,
    pub raw: Value,
}

/// Query string pairs.
pub type Query<'a> = &'a [(&'a str, String)];

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    /// Server URL as configured (for messages).
    server: String,
    token: String,
}

impl Client {
    /// `server` is a normalized http(s) URL (see `config::normalize_server`).
    pub fn new(server: &str, token: &str) -> Result<Self> {
        let base = Url::parse(server).with_context(|| format!("invalid server URL '{server}'"))?;
        if base.cannot_be_a_base() || !matches!(base.scheme(), "http" | "https") {
            anyhow::bail!("invalid server URL '{server}': expected http(s)://host[:port]");
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("ferry-cli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .context("initializing HTTP client")?;
        Ok(Client { http, base, server: server.trim_end_matches('/').to_string(), token: token.to_string() })
    }

    /// The server URL (no trailing slash).
    pub fn server(&self) -> &str {
        &self.server
    }

    /// `<server>/api/v1/<segments...>?<query>` with each segment percent-encoded.
    pub fn api_url(&self, segments: &[&str], query: Query<'_>) -> Result<Url> {
        let mut url = self.base.clone();
        for s in segments {
            if s.is_empty() || *s == "." || *s == ".." {
                anyhow::bail!("invalid name or id '{s}'");
            }
        }
        {
            let mut path = url.path_segments_mut().map_err(|_| anyhow!("invalid server URL '{}'", self.server))?;
            path.pop_if_empty();
            path.extend(["api", "v1"]);
            path.extend(segments);
        }
        url.set_query(None);
        if !query.is_empty() {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
        }
        Ok(url)
    }

    /// Turn a transport-level failure into a friendly message.
    fn transport_error(&self, err: reqwest::Error, url: &Url) -> anyhow::Error {
        if err.is_connect() {
            if connection_refused(&err) {
                return anyhow!("cannot reach Ferry server at {} — is ferryd running?", self.server);
            }
            return anyhow!("cannot reach Ferry server at {}: {}", self.server, root_cause(&err));
        }
        if err.is_timeout() {
            return anyhow!("request to {} timed out", redact(url));
        }
        anyhow!("request to {} failed: {}", redact(url), root_cause(&err))
    }

    async fn send(&self, rb: RequestBuilder, url: &Url) -> Result<Response> {
        let resp = rb.bearer_auth(&self.token).send().await.map_err(|e| self.transport_error(e, url))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let body = resp.bytes().await.unwrap_or_default();
        Err(ApiError::from_body(status, &body).into())
    }

    async fn decode<T: DeserializeOwned>(&self, resp: Response, method: &Method, url: &Url) -> Result<Json<T>> {
        let bytes = resp.bytes().await.map_err(|e| self.transport_error(e, url))?;
        let raw: Value = if bytes.iter().all(u8::is_ascii_whitespace) {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .with_context(|| format!("unexpected (non-JSON) response from {method} {}", url.path()))?
        };
        let data = serde_json::from_value(raw.clone())
            .with_context(|| format!("unexpected response from {method} {}", url.path()))?;
        Ok(Json { data, raw })
    }

    /// Send a request (optional JSON body) and decode the JSON answer.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        query: Query<'_>,
        body: Option<Vec<u8>>,
    ) -> Result<Json<T>> {
        let url = self.api_url(segments, query)?;
        let mut rb =
            self.http.request(method.clone(), url.clone()).timeout(REQUEST_TIMEOUT).header(ACCEPT, "application/json");
        if let Some(b) = body {
            rb = rb.header(CONTENT_TYPE, "application/json").body(b);
        }
        let resp = self.send(rb, &url).await?;
        self.decode(resp, &method, &url).await
    }

    pub async fn get<T: DeserializeOwned>(&self, segments: &[&str], query: Query<'_>) -> Result<Json<T>> {
        self.request(Method::GET, segments, query, None).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: Query<'_>,
        body: &B,
    ) -> Result<Json<T>> {
        self.request(Method::POST, segments, query, Some(to_json(body)?)).await
    }

    /// POST without a body (actions such as restart, suspend, cancel).
    pub async fn post_empty<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<Json<T>> {
        self.request(Method::POST, segments, &[], None).await
    }

    pub async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: Query<'_>,
        body: &B,
    ) -> Result<Json<T>> {
        self.request(Method::PATCH, segments, query, Some(to_json(body)?)).await
    }

    /// DELETE returning a JSON body.
    pub async fn delete<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<Json<T>> {
        self.request(Method::DELETE, segments, &[], None).await
    }

    /// DELETE expecting no content (204); any body is ignored.
    pub async fn delete_no_content(&self, segments: &[&str], query: Query<'_>) -> Result<()> {
        let url = self.api_url(segments, query)?;
        let rb = self.http.delete(url.clone()).timeout(REQUEST_TIMEOUT);
        self.send(rb, &url).await?;
        Ok(())
    }

    /// Stream a file as the request body (`Content-Type: application/gzip`).
    pub async fn upload<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: Query<'_>,
        path: &Path,
    ) -> Result<Json<T>> {
        let url = self.api_url(segments, query)?;
        let file = tokio::fs::File::open(path).await.with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata().await.with_context(|| format!("reading {}", path.display()))?.len();
        let rb = self
            .http
            .post(url.clone())
            .header(CONTENT_TYPE, "application/gzip")
            .header(CONTENT_LENGTH, len)
            .header(ACCEPT, "application/json")
            .body(reqwest::Body::from(file));
        let resp = self.send(rb, &url).await?;
        self.decode(resp, &Method::POST, &url).await
    }

    /// Open an SSE stream (no overall timeout).
    pub async fn events(&self, segments: &[&str], query: Query<'_>) -> Result<EventStream> {
        let url = self.api_url(segments, query)?;
        let rb = self.http.get(url.clone()).header(ACCEPT, "text/event-stream");
        let resp = self.send(rb, &url).await?;
        Ok(EventStream { resp, parser: SseParser::new(), pending: VecDeque::new(), done: false, url })
    }
}

fn to_json<B: Serialize>(body: &B) -> Result<Vec<u8>> {
    serde_json::to_vec(body).context("serializing request body")
}

/// No bytes (not even keep-alive comments) arrived on an event stream for
/// the given idle period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleTimeout(pub Duration);

impl fmt::Display for IdleTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no data from the server for {}s", self.0.as_secs())
    }
}

impl std::error::Error for IdleTimeout {}

/// An open Server-Sent Events response.
pub struct EventStream {
    resp: Response,
    parser: SseParser,
    pending: VecDeque<SseEvent>,
    done: bool,
    url: Url,
}

impl EventStream {
    /// Next event; `Ok(None)` when the server closed the stream.
    /// With `idle`, fails with [`IdleTimeout`] if no bytes at all (not even
    /// keep-alive comments) arrive for that long.
    pub async fn next(&mut self, idle: Option<Duration>) -> Result<Option<SseEvent>> {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Ok(Some(ev));
            }
            if self.done {
                return Ok(None);
            }
            let chunk = match idle {
                Some(d) => match tokio::time::timeout(d, self.resp.chunk()).await {
                    Ok(r) => r,
                    Err(_) => return Err(IdleTimeout(d).into()),
                },
                None => self.resp.chunk().await,
            };
            match chunk {
                Ok(Some(bytes)) => self.pending.extend(self.parser.feed(&bytes)),
                Ok(None) => self.done = true,
                Err(e) => return Err(anyhow!("log stream from {} interrupted: {}", redact(&self.url), root_cause(&e))),
            }
        }
    }
}

/// URL without its query string (which may contain tokens) for messages.
fn redact(url: &Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.to_string()
}

fn connection_refused(err: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if let Some(io) = e.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::ConnectionRefused
        {
            return true;
        }
        source = e.source();
    }
    false
}

/// The innermost error message of a chain (the most informative one for
/// reqwest/hyper errors).
fn root_cause(err: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = err;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(server: &str) -> Client {
        Client::new(server, "t").unwrap()
    }

    #[test]
    fn api_urls_encode_segments_and_query() {
        let c = client("http://127.0.0.1:7878");
        let u = c.api_url(&["services", "my app/x?#%", "deploys"], &[("limit", "20".into())]).unwrap();
        assert_eq!(u.as_str(), "http://127.0.0.1:7878/api/v1/services/my%20app%2Fx%3F%23%25/deploys?limit=20");
        let u = c.api_url(&["services", "web", "logs"], &[("follow", "true".into()), ("tail", "5".into())]).unwrap();
        assert_eq!(u.as_str(), "http://127.0.0.1:7878/api/v1/services/web/logs?follow=true&tail=5");
    }

    #[test]
    fn api_urls_keep_a_base_path() {
        let c = client("https://example.com/ferry");
        let u = c.api_url(&["info"], &[]).unwrap();
        assert_eq!(u.as_str(), "https://example.com/ferry/api/v1/info");
        let c = client("https://example.com/ferry/");
        assert_eq!(c.api_url(&["info"], &[]).unwrap().as_str(), "https://example.com/ferry/api/v1/info");
        assert_eq!(c.server(), "https://example.com/ferry");
    }

    #[test]
    fn dot_segments_are_rejected() {
        let c = client("http://localhost:7878");
        assert!(c.api_url(&["services", ".."], &[]).is_err());
        assert!(c.api_url(&["services", "."], &[]).is_err());
        assert!(c.api_url(&["services", ""], &[]).is_err());
    }

    #[test]
    fn api_error_parsing() {
        let e = ApiError::from_body(404, br#"{"error":{"code":"not_found","message":"service 'x' not found"}}"#);
        assert_eq!(e.to_string(), "service 'x' not found (not_found)");
        let e = ApiError::from_body(502, b"Bad Gateway from upstream");
        assert_eq!(e.code, "http_502");
        assert_eq!(e.to_string(), "server returned 502 Bad Gateway: Bad Gateway from upstream (http_502)");
        let e = ApiError::from_body(500, b"");
        assert_eq!(e.message, "server returned 500 Internal Server Error");
        let any: anyhow::Error = ApiError::from_body(404, b"").into();
        assert!(is_not_found(&any));
        assert!(is_not_found(&any.context("while doing x")));
    }

    #[test]
    fn rejects_non_http_servers() {
        assert!(Client::new("ftp://x", "t").is_err());
        assert!(Client::new("not a url", "t").is_err());
    }
}
