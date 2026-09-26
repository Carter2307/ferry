//! Routing, headers, error pages, retries and ACME over real sockets.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use http::Request;
use http_body_util::Full;

use super::support::*;

fn get(path: &str, host: &str) -> Request<Full<Bytes>> {
    Request::get(path).header("host", host).body(Full::default()).unwrap()
}

#[tokio::test]
async fn routes_by_host_with_case_and_port_normalization() {
    let a = echo_upstream("a").await;
    let b = echo_upstream("b").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv-a", &["app.test"], &[a.addr]);
    proxy.route("srv-b", &["Other.Test."], &[b.addr]);

    let host = format!("APP.Test.:{}", proxy.http.port());
    let resp = h1_send(proxy.http, get("/hello?x=1&y=%20", &host)).await;
    assert_eq!(resp.status, 200);
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.upstream, "a");
    assert_eq!(echo.method, "GET");
    assert_eq!(echo.path, "/hello?x=1&y=%20");
    assert_eq!(echo.version, "HTTP/1.1");
    // The original Host header (case, trailing dot and port) reaches the app.
    assert_eq!(echo.header("host"), Some(host.as_str()));

    let resp = h1_send(proxy.http, get("/", "other.test")).await;
    assert_eq!(Echo::parse(&resp.body).upstream, "b");

    // Absolute-form targets route by their authority (RFC 9112 §3.2.2).
    let target = format!("http://other.test:{}/abs", proxy.http.port());
    let resp = h1_send(proxy.http, get(&target, "app.test")).await;
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.upstream, "b");
    assert_eq!(echo.path, "/abs");
    assert_eq!(echo.header("host"), Some(format!("other.test:{}", proxy.http.port()).as_str()));
}

#[tokio::test]
async fn round_robins_across_upstreams() {
    let a = echo_upstream("a").await;
    let b = echo_upstream("b").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[a.addr, b.addr]);
    let c = client();
    let mut seen = Vec::new();
    for _ in 0..6 {
        let r = c.get(proxy.url("app.test", "/")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        seen.push(r.headers()["x-upstream"].to_str().unwrap().to_string());
    }
    assert_eq!(seen, ["a", "b", "a", "b", "a", "b"]);
}

#[tokio::test]
async fn error_pages() {
    let proxy = TestProxy::start().await;
    proxy.routes.set_service_suspended("sus", &["sus.test".to_string()]);
    proxy.route("empty", &["empty.test"], &[]);
    proxy.route("dead", &["dead.test"], &[dead_addr().await]);
    let c = client_for(&["unknown.test", "sus.test", "empty.test", "dead.test"]).build().unwrap();

    let cases = [
        ("unknown.test", 404, "not_found", "No service is configured for unknown.test"),
        ("sus.test", 503, "suspended", "This service is suspended"),
        ("empty.test", 503, "no_upstreams", "No healthy instances yet — a deploy may be in progress"),
        ("dead.test", 502, "bad_gateway", "Bad gateway: the service is not responding"),
    ];
    for (host, status, kind, message) in cases {
        let r = c.get(proxy.url(host, "/some/path")).send().await.unwrap();
        assert_eq!(r.status(), status, "{host}");
        assert_eq!(r.headers()["x-ferry-error"], kind, "{host}");
        assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
        let retry_after = r.headers().get("retry-after").map(|v| v.to_str().unwrap().to_string());
        assert_eq!(retry_after.as_deref(), (kind == "no_upstreams").then_some("5"), "{host}");
        let body = r.text().await.unwrap();
        assert!(body.contains(message), "{host}: {body}");
        assert!(body.contains("Ferry"));
        assert!(body.contains(&status.to_string()));

        let r = c.head(proxy.url(host, "/some/path")).send().await.unwrap();
        assert_eq!(r.status(), status);
        assert_eq!(r.headers()["x-ferry-error"], kind);
        // Same length a GET would get, but no body.
        assert_eq!(r.headers()["content-length"], body.len().to_string().as_str(), "{host}");
        assert!(r.bytes().await.unwrap().is_empty());
    }

    // A POST with a body to a dead upstream is not retried either.
    let r = c.post(proxy.url("dead.test", "/")).body("payload").send().await.unwrap();
    assert_eq!(r.status(), 502);
}

#[tokio::test]
async fn host_is_escaped_in_404_page() {
    let proxy = TestProxy::start().await;
    // `'` and `&` are valid in an authority but must not break the HTML.
    let resp = h1_send(proxy.http, get("/", "a&b'c.test")).await;
    assert_eq!(resp.status, 404);
    assert!(resp.body.contains("No service is configured for a&amp;b&#39;c.test"), "{}", resp.body);
}

#[tokio::test]
async fn bad_requests() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    // HTTP/1.0 without Host.
    let out = raw_exchange(proxy.http, "GET / HTTP/1.0\r\n\r\n").await;
    assert!(out.contains(" 400 "), "{out}");
    assert!(out.to_ascii_lowercase().contains("x-ferry-error: bad_request"), "{out}");

    // Invalid and duplicate Host headers.
    for request in [
        "GET / HTTP/1.1\r\nHost: bad host\r\nConnection: close\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: user@app.test\r\nConnection: close\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: app.test\r\nHost: other.test\r\nConnection: close\r\n\r\n",
    ] {
        let out = raw_exchange(proxy.http, request).await;
        assert!(out.starts_with("HTTP/1.1 400"), "{request:?} → {out}");
    }

    // CONNECT is not forwarded.
    let out =
        raw_exchange(proxy.http, "CONNECT app.test:443 HTTP/1.1\r\nHost: app.test:443\r\nConnection: close\r\n\r\n")
            .await;
    assert!(out.starts_with("HTTP/1.1 405"), "{out}");
}

#[tokio::test]
async fn options_asterisk_is_forwarded() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let out = raw_exchange(proxy.http, "OPTIONS * HTTP/1.1\r\nHost: app.test\r\nConnection: close\r\n\r\n").await;
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(out.contains("method=OPTIONS\npath=*\n"), "{out}");
}

#[tokio::test]
async fn set_service_routes_swaps_and_replaces_hosts() {
    let blue = echo_upstream("blue").await;
    let green = echo_upstream("green").await;
    let proxy = TestProxy::start().await;
    let c = client_for(&["swap.test", "new.test"]).build().unwrap();
    let upstream_of = |r: &reqwest::Response| r.headers().get("x-upstream").map(|v| v.to_str().unwrap().to_string());

    proxy.route("srv", &["swap.test"], &[blue.addr]);
    let r = c.get(proxy.url("swap.test", "/")).send().await.unwrap();
    assert_eq!(upstream_of(&r).as_deref(), Some("blue"));

    // Blue/green swap on the same host.
    proxy.route("srv", &["swap.test"], &[green.addr]);
    let r = c.get(proxy.url("swap.test", "/")).send().await.unwrap();
    assert_eq!(upstream_of(&r).as_deref(), Some("green"));

    // New host set: the old host is gone.
    proxy.route("srv", &["new.test"], &[green.addr]);
    let r = c.get(proxy.url("swap.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = c.get(proxy.url("new.test", "/")).send().await.unwrap();
    assert_eq!(upstream_of(&r).as_deref(), Some("green"));

    proxy.routes.remove_service("srv");
    let r = c.get(proxy.url("new.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn forwarded_headers_and_hop_by_hop_stripping() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let host = format!("app.test:{}", proxy.http.port());

    let req = Request::post("/hop?q=1")
        .header("host", &host)
        .header("x-forwarded-for", "10.9.8.7")
        .header("x-forwarded-proto", "https")
        .header("x-forwarded-host", "evil.example")
        .header("x-real-ip", "6.6.6.6")
        // Spoofed forwarding headers some frameworks read first.
        .header("forwarded", "for=6.6.6.6;proto=https;host=evil.example")
        .header("x-forwarded-ssl", "on")
        .header("x-forwarded-scheme", "https")
        .header("x-client-ip", "6.6.6.6")
        .header("connection", "x-drop-me")
        .header("x-drop-me", "1")
        .header("keep-alive", "timeout=1")
        .header("proxy-authorization", "Basic Zm9v")
        .header("proxy-connection", "keep-alive")
        .header("te", "trailers")
        .header("x-request-id", "req-123")
        .header("x-custom", "kept")
        .body(Full::from("hello"))
        .unwrap();
    let resp = h1_send(proxy.http, req).await;
    assert_eq!(resp.status, 200);
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.method, "POST");
    assert_eq!(echo.path, "/hop?q=1");
    assert_eq!(echo.body_bytes, 5);
    assert_eq!(echo.header("host"), Some(host.as_str()));
    // The edge only vouches for the address it saw.
    assert_eq!(echo.headers["x-forwarded-for"], ["127.0.0.1"]);
    assert_eq!(echo.header("x-forwarded-proto"), Some("http"));
    assert_eq!(echo.headers["x-forwarded-host"], [host.as_str()]);
    assert_eq!(echo.header("x-forwarded-port"), Some(proxy.http.port().to_string().as_str()));
    assert_eq!(echo.header("x-real-ip"), Some("127.0.0.1"));
    assert_eq!(echo.headers["forwarded"], [format!("for=127.0.0.1;host=\"{host}\";proto=http")]);
    assert_eq!(echo.header("x-request-id"), Some("req-123"));
    assert_eq!(echo.header("x-custom"), Some("kept"));
    for gone in [
        "x-drop-me",
        "keep-alive",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "x-forwarded-ssl",
        "x-forwarded-scheme",
        "x-client-ip",
    ] {
        assert_eq!(echo.header(gone), None, "{gone} reached the upstream");
    }
    // Response hop-by-hop headers don't reach the client.
    assert_eq!(resp.headers["x-end-to-end"], "kept");
    for gone in ["x-up-hop", "keep-alive", "proxy-authenticate"] {
        assert!(resp.headers.get(gone).is_none(), "{gone} reached the client");
    }

    // A request id is generated when absent.
    let resp = h1_send(proxy.http, get("/", &host)).await;
    let echo = Echo::parse(&resp.body);
    let id = echo.header("x-request-id").unwrap();
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(echo.header("x-forwarded-for"), Some("127.0.0.1"));
}

#[tokio::test]
async fn http2_client_requests_become_http11_upstream() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["h2.test"], &[up.addr]);
    let authority = format!("h2.test:{}", proxy.http.port());
    let req = Request::post(format!("http://{authority}/h2?x=1"))
        .header("cookie", "a=1")
        .header("cookie", "b=2")
        .body(Full::from("h2 body"))
        .unwrap();
    let resp = h2c_send(proxy.http, req).await;
    assert_eq!(resp.status, 200);
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.version, "HTTP/1.1");
    assert_eq!(echo.path, "/h2?x=1");
    assert_eq!(echo.header("host"), Some(authority.as_str()));
    assert_eq!(echo.header("x-forwarded-host"), Some(authority.as_str()));
    assert_eq!(echo.headers["cookie"], vec!["a=1; b=2"]);
    assert_eq!(echo.body_bytes, 7);
}

#[tokio::test]
async fn retries_bodyless_get_on_another_upstream() {
    let alive = echo_upstream("alive").await;
    let dead = dead_addr().await;
    let proxy = TestProxy::start().await;
    // Round robin starts at the first upstream: the dead one.
    proxy.route("srv", &["retry.test"], &[dead, alive.addr]);
    proxy.route("srv-post", &["post.test"], &[dead, alive.addr]);
    proxy.route("srv-single", &["single.test"], &[dead]);
    let c = client_for(&["retry.test", "post.test", "single.test"]).build().unwrap();

    for _ in 0..4 {
        let r = c.get(proxy.url("retry.test", "/")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["x-upstream"], "alive");
    }
    let r = c.head(proxy.url("retry.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 200);

    // A request with a body can't be replayed.
    let r = c.post(proxy.url("post.test", "/")).body("data").send().await.unwrap();
    assert_eq!(r.status(), 502);
    assert_eq!(r.headers()["x-ferry-error"], "bad_gateway");

    // No other upstream to try.
    let r = c.get(proxy.url("single.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 502);
}

#[tokio::test]
async fn upgrade_headers_are_forwarded_when_upstream_declines() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let req = Request::get("/ws")
        .header("host", "app.test")
        .header("connection", "keep-alive, Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Full::default())
        .unwrap();
    let resp = h1_send(proxy.http, req).await;
    // The echo server ignores the upgrade and answers 200.
    assert_eq!(resp.status, 200);
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.header("connection"), Some("upgrade"));
    assert_eq!(echo.header("upgrade"), Some("websocket"));
    assert_eq!(echo.header("sec-websocket-key"), Some("dGhlIHNhbXBsZSBub25jZQ=="));
}

/// HTTP/1.0 has no protocol upgrades: the headers are dropped and the
/// request proxied as usual (instead of a `101` followed by a dead tunnel).
#[tokio::test]
async fn http10_upgrade_requests_are_proxied_as_plain_http() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let out = raw_exchange(
        proxy.http,
        "GET /ws HTTP/1.0\r\nHost: app.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
    )
    .await;
    assert!(out.starts_with("HTTP/1.0 200"), "{out}");
    assert!(out.contains("method=GET\npath=/ws\n"), "{out}");
    assert!(!out.contains("header:upgrade="), "{out}");
    assert!(!out.contains("header:connection="), "{out}");
}

#[tokio::test]
async fn h2c_upgrade_requests_are_proxied_as_plain_http() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let req = Request::get("/")
        .header("host", "app.test")
        .header("connection", "Upgrade, HTTP2-Settings")
        .header("upgrade", "h2c")
        .header("http2-settings", "AAMAAABkAAQAoAAAAAIAAAAA")
        .body(Full::default())
        .unwrap();
    let resp = h1_send(proxy.http, req).await;
    assert_eq!(resp.status, 200);
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.header("upgrade"), None);
    assert_eq!(echo.header("http2-settings"), None);
}

#[tokio::test]
async fn answers_acme_challenges() {
    let up = echo_upstream("a").await;
    let hooks = MockHooks {
        challenges: HashMap::from([("tok123".to_string(), "tok123.thumbprint".to_string())]),
        ..MockHooks::default()
    };
    let proxy = TestProxy::start_with(Options { hooks: Some(Arc::new(hooks)), ..Options::default() }).await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let c = client_for(&["app.test", "unrouted.test"]).build().unwrap();

    // Answered before routing, whatever the host.
    for host in ["app.test", "unrouted.test"] {
        let r = c.get(proxy.url(host, "/.well-known/acme-challenge/tok123")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["content-type"], "text/plain");
        assert_eq!(r.text().await.unwrap(), "tok123.thumbprint");
    }
    let r = c.head(proxy.url("app.test", "/.well-known/acme-challenge/tok123")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.bytes().await.unwrap().is_empty());

    let r = c.get(proxy.url("app.test", "/.well-known/acme-challenge/unknown")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-ferry-error"], "not_found");

    // Other paths (and other methods) still reach the app.
    let r = c.get(proxy.url("app.test", "/.well-known/other")).send().await.unwrap();
    assert_eq!(r.headers()["x-upstream"], "a");
    let r = c.post(proxy.url("app.test", "/.well-known/acme-challenge/tok123")).send().await.unwrap();
    assert_eq!(r.headers()["x-upstream"], "a");
}

#[tokio::test]
async fn acme_paths_pass_through_without_tls_hooks() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let r = client().get(proxy.url("app.test", "/.well-known/acme-challenge/x")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-upstream"], "a");
}

#[tokio::test]
async fn keep_alive_connections_serve_many_requests() {
    let a = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[a.addr]);
    let c = client();
    let mut handles = Vec::new();
    for i in 0..50 {
        let c = c.clone();
        let url = proxy.url("app.test", &format!("/n/{i}"));
        handles.push(tokio::spawn(async move { c.get(url).send().await.unwrap().text().await.unwrap() }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        let echo = Echo::parse(&h.await.unwrap());
        assert_eq!(echo.path, format!("/n/{i}"));
    }
}

/// GET bodies without a length (HTTP/1 chunked, HTTP/2 without
/// content-length) are forwarded chunked, not silently dropped.
#[tokio::test]
async fn get_bodies_of_unknown_length_reach_the_upstream() {
    let up = echo_upstream("a").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    for method in ["GET", "DELETE"] {
        let out = raw_exchange(
            proxy.http,
            &format!(
                "{method} /getbody HTTP/1.1\r\nHost: app.test\r\nTransfer-Encoding: chunked\r\n\
                 Connection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n"
            ),
        )
        .await;
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(out.contains(&format!("method={method}\n")), "{out}");
        assert!(out.contains("header:transfer-encoding=chunked\n"), "{out}");
        assert!(out.contains("body-bytes=5\n"), "{method}: {out}");
    }

    // HTTP/2 GET with a streamed body and no content-length.
    let stream = tokio::net::TcpStream::connect(proxy.http).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http2::handshake(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(stream),
    )
    .await
    .unwrap();
    tokio::spawn(conn);
    let chunks = futures::stream::iter(
        [&b"hello-"[..], &b"body"[..]]
            .map(|c| Ok::<_, std::convert::Infallible>(hyper::body::Frame::data(Bytes::from_static(c)))),
    );
    let req = Request::get(format!("http://app.test:{}/g", proxy.http.port()))
        .body(http_body_util::StreamBody::new(chunks))
        .unwrap();
    let resp = collect(within("h2 request", sender.send_request(req)).await.unwrap()).await;
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.method, "GET");
    assert_eq!(echo.body_bytes, 10);

    // Bodyless GETs stay bodyless (and replayable).
    let resp = h1_send(proxy.http, get("/", "app.test")).await;
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.body_bytes, 0);
    assert_eq!(echo.header("transfer-encoding"), None);
    assert_eq!(echo.header("content-length"), None);
}

/// An upstream response framed by `Transfer-Encoding` keeps its whole body:
/// a `Content-Length` sent along with it is stale and dropped.
#[tokio::test]
async fn stale_upstream_content_length_is_dropped() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n\
                            5\r\nhello\r\n0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });
    let proxy = TestProxy::start().await;
    proxy.route("raw", &["raw.test"], &[addr]);

    let resp = h1_send(proxy.http, get("/tecl", "raw.test")).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "hello");
    assert!(resp.headers.get("content-length").is_none(), "{:?}", resp.headers);
    upstream.abort();
}
