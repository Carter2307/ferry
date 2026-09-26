//! A scripted stand-in for the Docker daemon (plain HTTP on 127.0.0.1), so
//! unit tests can check the requests `Docker` sends and how it maps the
//! daemon's answers — without a real daemon.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One request received by the fake daemon.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: String,
    /// Path and query, without the `/v1.xx` API version prefix.
    pub target: String,
    pub body: String,
}

impl Request {
    /// The body parsed as JSON (`Null` when empty).
    pub fn json(&self) -> serde_json::Value {
        if self.body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("request body is not JSON ({e}): {}", self.body))
        }
    }
}

type Responder = dyn Fn(&Request) -> (u16, String) + Send + Sync;

pub(crate) struct FakeDaemon {
    /// A client talking to this daemon.
    pub docker: crate::Docker,
    requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeDaemon {
    /// Answer every request with `respond(request) -> (status, JSON body)`.
    pub async fn start(respond: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind the fake daemon");
        let addr = listener.local_addr().expect("fake daemon address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let respond: Arc<Responder> = Arc::new(respond);
        let log = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&respond), Arc::clone(&log)));
            }
        });
        let inner = bollard::Docker::connect_with_http(&format!("http://{addr}"), 10, bollard::API_DEFAULT_VERSION)
            .expect("client for the fake daemon");
        FakeDaemon { docker: crate::Docker::from_bollard(inner), requests, task }
    }

    /// Requests received so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("request log").clone()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One request per connection (the client does not keep connections alive).
async fn serve(mut stream: TcpStream, respond: Arc<Responder>, log: Arc<Mutex<Vec<Request>>>) {
    let Some(request) = read_request(&mut stream).await else { return };
    let (status, body) = respond(&request);
    log.lock().expect("request log").push(request);
    let response = format!(
        "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..]).into_owned();
    Some(Request { method, target: strip_api_version(&target).to_string(), body })
}

/// `/v1.51/containers/x/exec` → `/containers/x/exec` (other paths unchanged).
fn strip_api_version(target: &str) -> &str {
    let Some(rest) = target.strip_prefix("/v") else { return target };
    let Some(slash) = rest.find('/') else { return target };
    let is_version = rest[..slash].split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    if is_version { &rest[slash..] } else { target }
}

#[test]
fn strips_api_versions() {
    assert_eq!(strip_api_version("/v1.51/containers/x/exec"), "/containers/x/exec");
    assert_eq!(strip_api_version("/volumes/data"), "/volumes/data");
    assert_eq!(strip_api_version("/v1.51"), "/v1.51");
    assert_eq!(strip_api_version("/images/x/tag?repo=a"), "/images/x/tag?repo=a");
}
