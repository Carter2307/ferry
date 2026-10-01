//! Test support: a tiny fake Ferry API (axum) that returns canned replies and
//! records every request, plus a runner for the real `ferry` binary.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};

pub const TOKEN: &str = "test-token";

/// A request received by the fake server.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// Raw (still percent-encoded) path.
    pub path: String,
    pub query: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Recorded {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    }
}

/// A canned reply; SSE replies are streamed chunk by chunk.
#[derive(Debug, Clone)]
pub struct Reply {
    status: u16,
    content_type: &'static str,
    chunks: Vec<Bytes>,
    /// Keep an event stream open (silent) after the last chunk.
    hold_open: bool,
}

impl Reply {
    pub fn json(status: u16, value: Value) -> Self {
        Reply {
            status,
            content_type: "application/json",
            chunks: vec![Bytes::from(value.to_string())],
            hold_open: false,
        }
    }

    pub fn ok(value: Value) -> Self {
        Self::json(200, value)
    }

    pub fn error(status: u16, code: &str, message: &str) -> Self {
        Self::json(status, json!({ "error": { "code": code, "message": message } }))
    }

    pub fn no_content() -> Self {
        Reply { status: 204, content_type: "text/plain", chunks: Vec::new(), hold_open: false }
    }

    /// An event stream sent in exactly these chunks (with small pauses, so
    /// the client really sees them separately).
    pub fn sse(chunks: &[&str]) -> Self {
        Reply {
            status: 200,
            content_type: "text/event-stream",
            chunks: chunks.iter().map(|c| Bytes::from(c.to_string())).collect(),
            hold_open: false,
        }
    }

    /// Like [`Reply::sse`], but the stream then stays open without data.
    pub fn sse_open(chunks: &[&str]) -> Self {
        Reply { hold_open: true, ..Self::sse(chunks) }
    }
}

struct Route {
    method: String,
    path: String,
    /// Replies in order; the last one repeats.
    replies: VecDeque<Reply>,
}

#[derive(Default)]
pub struct Fake {
    routes: Mutex<Vec<Route>>,
    log: Mutex<Vec<Recorded>>,
}

impl Fake {
    /// Start on an ephemeral port; returns the server and its base URL.
    pub async fn start() -> (Arc<Fake>, String) {
        let fake = Arc::new(Fake::default());
        let app = axum::Router::new().fallback(handle).with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (fake, format!("http://{addr}"))
    }

    /// Register a reply for `METHOD path` (raw path, no query). Several
    /// calls for the same route queue replies.
    pub fn on(&self, method: &str, path: &str, reply: Reply) {
        let mut routes = self.routes.lock().unwrap();
        if let Some(r) = routes.iter_mut().find(|r| r.method == method && r.path == path) {
            r.replies.push_back(reply);
        } else {
            routes.push(Route { method: method.into(), path: path.into(), replies: VecDeque::from([reply]) });
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.log.lock().unwrap().clone()
    }

    /// Requests matching `METHOD path`.
    pub fn find(&self, method: &str, path: &str) -> Vec<Recorded> {
        self.requests().into_iter().filter(|r| r.method == method && r.path == path).collect()
    }

    fn reply_for(&self, method: &str, path: &str) -> Option<Reply> {
        let mut routes = self.routes.lock().unwrap();
        let route = routes.iter_mut().find(|r| r.method == method && r.path == path)?;
        if route.replies.len() > 1 { route.replies.pop_front() } else { route.replies.front().cloned() }
    }
}

async fn handle(State(fake): State<Arc<Fake>>, req: Request) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), usize::MAX).await.unwrap_or_default();
    fake.log.lock().unwrap().push(Recorded {
        method: method.clone(),
        path: path.clone(),
        query,
        headers: headers.clone(),
        body,
    });

    // What `ferry login` calls before it has a token needs none.
    let public = path.starts_with("/api/v1/auth/cli");
    let authorized =
        headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"));
    let reply = if !authorized && !public {
        Reply::error(401, "unauthorized", "invalid or missing API token")
    } else {
        fake.reply_for(&method, &path)
            .unwrap_or_else(|| Reply::error(404, "not_found", &format!("no route {method} {path}")))
    };
    let chunks = reply.chunks;
    let hold_open = reply.hold_open;
    let body = if reply.content_type == "text/event-stream" {
        let stream = futures::stream::unfold(VecDeque::from(chunks), move |mut q| async move {
            let Some(chunk) = q.pop_front() else {
                if hold_open {
                    futures::future::pending::<()>().await;
                }
                return None;
            };
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some((Ok::<_, Infallible>(chunk), q))
        });
        Body::from_stream(stream)
    } else {
        Body::from(chunks.concat())
    };
    Response::builder()
        .status(StatusCode::from_u16(reply.status).unwrap())
        .header(header::CONTENT_TYPE, reply.content_type)
        .body(body)
        .unwrap()
}

/// Result of running the binary.
#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A `ferry` command with an isolated config dir. `server`/`token` set
/// `FERRY_SERVER`/`FERRY_TOKEN` when given.
pub fn command(
    cwd: Option<&Path>,
    home: &Path,
    server: Option<&str>,
    token: Option<&str>,
    args: &[&str],
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_ferry"));
    cmd.args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for var in [
        "FERRY_SERVER",
        "FERRY_TOKEN",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    if let Some(s) = server {
        cmd.env("FERRY_SERVER", s);
    }
    if let Some(t) = token {
        cmd.env("FERRY_TOKEN", t);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd
}

/// Run `ferry` to completion (see [`command`]).
pub async fn ferry_in(
    cwd: Option<&Path>,
    home: &Path,
    server: Option<&str>,
    token: Option<&str>,
    args: &[&str],
) -> Output {
    let mut cmd = command(cwd, home, server, token, args);
    let out = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .expect("ferry did not finish within 120s")
        .expect("running ferry");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Run `ferry` against `server` with the test token.
pub async fn ferry(server: &str, home: &Path, args: &[&str]) -> Output {
    ferry_in(None, home, Some(server), Some(TOKEN), args).await
}
