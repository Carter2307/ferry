//! Websocket upgrades are spliced through the proxy.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request as WsRequest, Response as WsResponse};

use super::support::*;

type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Websocket echo server recording the handshake request's headers and path.
async fn ws_echo_upstream() -> (SocketAddr, Seen, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
    let task = tokio::spawn({
        let seen = seen.clone();
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                let seen = seen.clone();
                tokio::spawn(async move {
                    // The error type is fixed by tungstenite's `Callback` signature.
                    #[allow(clippy::result_large_err)]
                    let callback = move |req: &WsRequest, resp: WsResponse| {
                        let mut seen = seen.lock().unwrap();
                        seen.push((":path".into(), req.uri().to_string()));
                        for (k, v) in req.headers() {
                            seen.push((k.to_string(), v.to_str().unwrap_or("").to_string()));
                        }
                        Ok(resp)
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };
                    while let Some(Ok(msg)) = ws.next().await {
                        if msg.is_text() || msg.is_binary() {
                            if ws.send(msg).await.is_err() {
                                break;
                            }
                        } else if msg.is_close() {
                            break;
                        }
                    }
                });
            }
        }
    });
    (addr, seen, task)
}

fn seen_header(seen: &Seen, name: &str) -> Option<String> {
    seen.lock().unwrap().iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
}

#[tokio::test]
async fn websocket_echo_through_proxy() {
    let (addr, seen, upstream) = ws_echo_upstream().await;
    let proxy = TestProxy::start().await;
    proxy.route("ws", &["ws.test"], &[addr]);

    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let url = format!("ws://ws.test:{}/chat?room=1", proxy.http.port());
    let (mut ws, resp) = within("websocket handshake", tokio_tungstenite::client_async(url, stream)).await.unwrap();
    assert_eq!(resp.status(), 101);
    assert!(resp.headers().get("sec-websocket-accept").is_some());

    ws.send(Message::text("hello ferry")).await.unwrap();
    let msg = within("text echo", ws.next()).await.unwrap().unwrap();
    assert_eq!(msg.to_text().unwrap(), "hello ferry");

    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 256) as u8).collect();
    ws.send(Message::binary(payload.clone())).await.unwrap();
    let msg = within("binary echo", ws.next()).await.unwrap().unwrap();
    assert_eq!(msg.into_data().as_ref(), payload.as_slice());

    ws.close(None).await.unwrap();

    assert_eq!(seen_header(&seen, ":path").as_deref(), Some("/chat?room=1"));
    assert_eq!(seen_header(&seen, "host"), Some(format!("ws.test:{}", proxy.http.port())));
    assert_eq!(seen_header(&seen, "x-forwarded-for").as_deref(), Some("127.0.0.1"));
    assert_eq!(seen_header(&seen, "upgrade").as_deref(), Some("websocket"));
    upstream.abort();
}

#[tokio::test]
async fn many_concurrent_websockets() {
    let (addr, _seen, upstream) = ws_echo_upstream().await;
    let proxy = TestProxy::start().await;
    proxy.route("ws", &["ws.test"], &[addr]);
    let port = proxy.http.port();
    let http = proxy.http;
    let clients: Vec<_> = (0..20)
        .map(|i| {
            tokio::spawn(async move {
                let stream = TcpStream::connect(http).await.unwrap();
                let (mut ws, _) =
                    tokio_tungstenite::client_async(format!("ws://ws.test:{port}/"), stream).await.unwrap();
                for j in 0..10 {
                    let text = format!("client {i} message {j}");
                    ws.send(Message::text(text.clone())).await.unwrap();
                    let msg = ws.next().await.unwrap().unwrap();
                    assert_eq!(msg.to_text().unwrap(), text);
                }
                ws.close(None).await.unwrap();
            })
        })
        .collect();
    for c in clients {
        within("websocket client", c).await.unwrap();
    }
    upstream.abort();
}

#[tokio::test]
async fn websocket_to_missing_service_gets_error_page() {
    let proxy = TestProxy::start().await;
    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let url = format!("ws://nobody.test:{}/", proxy.http.port());
    let err = within("handshake", tokio_tungstenite::client_async(url, stream)).await.unwrap_err();
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), 404);
            assert_eq!(resp.headers()["x-ferry-error"], "not_found");
        }
        other => panic!("unexpected error {other:?}"),
    }
}
