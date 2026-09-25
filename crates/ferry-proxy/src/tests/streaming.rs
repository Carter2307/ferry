//! Bodies flow through the proxy without buffering, in both directions.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::SinkExt;
use http::Response;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use tokio::sync::{Notify, oneshot};

use super::support::*;
use crate::body::{self, BoxError};

#[tokio::test]
async fn large_streamed_post_body() {
    const CHUNK: usize = 64 * 1024;
    const CHUNKS: usize = 512; // 32 MiB
    let up = echo_upstream("big").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    let chunk = |i: usize| Bytes::from(vec![(i % 251) as u8; CHUNK]);
    let expected = (0..CHUNKS).fold(FNV_START, |h, i| fnv(h, &chunk(i)));
    let stream = futures::stream::iter((0..CHUNKS).map(move |i| Ok::<_, std::io::Error>(chunk(i))));
    let r =
        client().post(proxy.url("app.test", "/upload")).body(reqwest::Body::wrap_stream(stream)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let echo = Echo::parse(&r.text().await.unwrap());
    assert_eq!(echo.body_bytes, (CHUNK * CHUNKS) as u64);
    assert_eq!(echo.body_fnv, format!("{expected:x}"));
    // Unknown length → re-chunked to the upstream.
    assert_eq!(echo.header("transfer-encoding"), Some("chunked"));
}

#[tokio::test]
async fn large_content_length_body() {
    let up = echo_upstream("big").await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let data = vec![7u8; 8 * 1024 * 1024 + 3];
    let r = client().put(proxy.url("app.test", "/")).body(data.clone()).send().await.unwrap();
    let echo = Echo::parse(&r.text().await.unwrap());
    assert_eq!(echo.body_bytes, data.len() as u64);
    assert_eq!(echo.header("content-length"), Some(data.len().to_string().as_str()));
    assert_eq!(echo.body_fnv, format!("{:x}", fnv(FNV_START, &data)));
}

/// The upstream sees the first request chunk while the client is still
/// sending: nothing is buffered in between.
#[tokio::test]
async fn request_body_is_streamed_to_upstream() {
    const FIRST: &[u8] = b"first-chunk";
    let (seen_tx, seen_rx) = oneshot::channel::<Bytes>();
    let seen_tx = Arc::new(Mutex::new(Some(seen_tx)));
    let up = spawn_upstream(move |req| {
        let seen_tx = seen_tx.clone();
        async move {
            let mut body = req.into_body();
            let mut received = Vec::new();
            while let Some(Ok(frame)) = body.frame().await {
                if let Ok(data) = frame.into_data() {
                    received.extend_from_slice(&data);
                }
                if received.len() >= FIRST.len()
                    && let Some(tx) = seen_tx.lock().unwrap().take()
                {
                    let _ = tx.send(Bytes::copy_from_slice(&received));
                }
            }
            Response::new(body::full(format!("total={}", received.len())))
        }
    })
    .await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let request = tokio::spawn(client().post(proxy.url("app.test", "/")).body(reqwest::Body::wrap_stream(rx)).send());
    tx.send(Ok(Bytes::from_static(FIRST))).await.unwrap();
    let seen = within("upstream receiving the first chunk", seen_rx).await.unwrap();
    assert_eq!(&seen[..], FIRST);
    tx.send(Ok(Bytes::from_static(b"-second"))).await.unwrap();
    drop(tx);
    let resp = within("response", request).await.unwrap().unwrap();
    assert_eq!(resp.text().await.unwrap(), format!("total={}", FIRST.len() + 7));
}

/// Server-sent events: the first event reaches the client before the
/// upstream has produced the rest of the response.
#[tokio::test]
async fn sse_response_is_streamed_to_client() {
    let release = Arc::new(Notify::new());
    let up = spawn_upstream({
        let release = release.clone();
        move |_req| {
            let release = release.clone();
            async move {
                let (mut tx, rx) = futures::channel::mpsc::channel::<Result<Frame<Bytes>, BoxError>>(4);
                tokio::spawn(async move {
                    let _ = tx.send(Ok(Frame::data(Bytes::from_static(b"data: first\n\n")))).await;
                    release.notified().await;
                    let _ = tx.send(Ok(Frame::data(Bytes::from_static(b"data: second\n\n")))).await;
                });
                Response::builder()
                    .header("content-type", "text/event-stream")
                    .header("cache-control", "no-cache")
                    .body(StreamBody::new(rx).boxed_unsync())
                    .unwrap()
            }
        }
    })
    .await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    let mut resp = within("SSE headers", client().get(proxy.url("app.test", "/events")).send()).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    assert!(resp.headers().get("content-length").is_none());
    let first = within("first SSE event", resp.chunk()).await.unwrap().unwrap();
    assert_eq!(&first[..], b"data: first\n\n");

    release.notify_one();
    let mut rest = Vec::new();
    while let Some(chunk) = within("remaining SSE events", resp.chunk()).await.unwrap() {
        rest.extend_from_slice(&chunk);
    }
    assert_eq!(rest, b"data: second\n\n");
}

/// Chunked responses whose chunks trickle in are forwarded chunk by chunk.
#[tokio::test]
async fn chunked_response_is_streamed_over_http2_too() {
    let release = Arc::new(Notify::new());
    let up = spawn_upstream({
        let release = release.clone();
        move |_req| {
            let release = release.clone();
            async move {
                let stream = futures::stream::unfold(0u8, move |i| {
                    let release = release.clone();
                    async move {
                        match i {
                            0 => Some((Ok::<_, BoxError>(Frame::data(Bytes::from_static(b"one"))), 1)),
                            1 => {
                                release.notified().await;
                                Some((Ok(Frame::data(Bytes::from_static(b"two"))), 2))
                            }
                            _ => None,
                        }
                    }
                });
                Response::new(StreamBody::new(stream).boxed_unsync())
            }
        }
    })
    .await;
    let proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    let stream = tokio::net::TcpStream::connect(proxy.http).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http2::handshake(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(stream),
    )
    .await
    .unwrap();
    tokio::spawn(conn);
    let req = http::Request::get(format!("http://app.test:{}/", proxy.http.port()))
        .body(http_body_util::Empty::<Bytes>::new())
        .unwrap();
    let resp = within("h2 response", sender.send_request(req)).await.unwrap();
    let mut body = resp.into_body();
    let first = within("first chunk", body.frame()).await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], b"one");
    release.notify_one();
    let mut rest = Vec::new();
    while let Some(frame) = within("rest", body.frame()).await {
        if let Ok(data) = frame.unwrap().into_data() {
            rest.extend_from_slice(&data);
        }
    }
    assert_eq!(rest, b"two");
}
