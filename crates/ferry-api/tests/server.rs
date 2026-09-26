//! The router behind a real TCP listener (`axum::serve`), driven with raw
//! HTTP/1.1 — checks streaming responses and request bodies end to end.

mod common;

use common::{TOKEN, TestApp};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn serve(app: &TestApp) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// Decode a `Transfer-Encoding: chunked` body.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(pos) = body.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(std::str::from_utf8(&body[..pos]).unwrap().trim(), 16).unwrap();
        body = &body[pos + 2..];
        if size == 0 {
            break;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
    out
}

/// Send a raw request (with `Connection: close`) and return (status, headers, body).
async fn raw(addr: std::net::SocketAddr, head: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(body).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let headers = String::from_utf8(buf[..split].to_vec()).unwrap();
    let mut body = buf[split + 4..].to_vec();
    if headers.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, headers, body)
}

#[tokio::test]
async fn real_socket_roundtrip() {
    let app = TestApp::new().await;
    let addr = serve(&app).await;

    let (status, _, body) = raw(addr, "GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", b"").await;
    assert_eq!((status, body.as_slice()), (200, &b"ok"[..]));

    // JSON create over the wire
    let payload = serde_json::to_vec(&json!({"name": "web", "image": "nginx"})).unwrap();
    let head = format!(
        "POST /api/v1/services HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let (status, _, body) = raw(addr, &head, &payload).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    let view: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let dep = view["latest_deploy"]["id"].as_str().unwrap().to_string();

    // SSE deploy logs with ?access_token= (EventSource style)
    let head = format!(
        "GET /api/v1/deploys/{dep}/logs?follow=true&access_token={TOKEN} HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n"
    );
    let (status, headers, body) = raw(addr, &head, b"").await;
    assert_eq!(status, 200);
    assert!(headers.to_ascii_lowercase().contains("content-type: text/event-stream"), "{headers}");
    let text = String::from_utf8(body).unwrap();
    assert_eq!(text.matches("event: log\n").count(), 2, "{text}");
    assert!(text.ends_with("event: end\ndata: \n\n"), "{text}");

    // chunked upload of a 1 MiB gzip-looking body
    let mut data = vec![0x1f, 0x8b];
    data.extend(std::iter::repeat_n(1u8, 1024 * 1024));
    let mut chunked = Vec::new();
    for part in data.chunks(64 * 1024) {
        chunked.extend(format!("{:x}\r\n", part.len()).as_bytes());
        chunked.extend(part);
        chunked.extend(b"\r\n");
    }
    chunked.extend(b"0\r\n\r\n");
    app.create_service(json!({"name": "up"})).await;
    let head = format!(
        "POST /api/v1/services/up/deploys/upload HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    let (status, _, body) = raw(addr, &head, &chunked).await;
    assert_eq!(status, 202, "{}", String::from_utf8_lossy(&body));
    let d: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let path = d["source"]["path"].as_str().unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len(), data.len() as u64);
}

/// Read from `s` until `needle` shows up in the accumulated (raw) response.
async fn read_until(s: &mut TcpStream, buf: &mut Vec<u8>, needle: &str) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !String::from_utf8_lossy(buf).contains(needle) {
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout_at(deadline, s.read(&mut chunk))
            .await
            .unwrap_or_else(|_| panic!("no {needle:?} within 5s: {}", String::from_utf8_lossy(buf)))
            .unwrap();
        assert!(n > 0, "connection closed before {needle:?}: {}", String::from_utf8_lossy(buf));
        buf.extend_from_slice(&chunk[..n]);
    }
}

#[tokio::test]
async fn change_feed_streams_over_a_real_socket_and_ends_on_shutdown() {
    let app = TestApp::new().await;
    let addr = serve(&app).await;
    let mut s = TcpStream::connect(addr).await.unwrap();
    let head =
        format!("GET /api/v1/events?access_token={TOKEN} HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n");
    s.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    read_until(&mut s, &mut buf, "event: ready\ndata: {}\n\n").await;
    let head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("content-type: text/event-stream"), "{head}");

    let v = app.create_service(json!({"name": "web"})).await;
    let id = v["id"].as_str().unwrap();
    read_until(&mut s, &mut buf, &format!(r#""kind":"service","id":"{id}""#)).await;

    // shutdown ends the response (the chunked body terminates)
    app.shutdown.cancel();
    read_until(&mut s, &mut buf, "\r\n0\r\n\r\n").await;
}
