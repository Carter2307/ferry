//! Connection limits and timeouts: connections that never send a request or
//! go idle are closed, requests in flight are never cut, and the connection
//! cap queues new clients while reclaiming idle connections.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http::{Request, Response};
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::{Body as _, Frame};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use super::support::*;
use crate::body::{self, BoxError};
use crate::{ConnectionLimits, server};

const SHORT: Duration = Duration::from_millis(300);
/// Upper bound for closing a connection idle for `SHORT`: an HTTP/2 peer
/// that never acknowledges the GOAWAY is only dropped after the close grace
/// (the rest close right away); the remainder is slack for loaded machines.
const CLOSED_BY: Duration = SHORT.saturating_add(server::CLOSE_GRACE).saturating_add(Duration::from_secs(2));

/// Every timeout at `SHORT`.
fn short_timeouts() -> ConnectionLimits {
    ConnectionLimits {
        header_read_timeout: SHORT,
        idle_timeout: SHORT,
        overload_idle_timeout: SHORT,
        ..ConnectionLimits::default()
    }
}

async fn proxy_with(limits: ConnectionLimits) -> TestProxy {
    TestProxy::start_with(Options { limits, ..Options::default() }).await
}

/// Read until the peer closes (EOF or reset); returns how long that took.
async fn closed_after<S: AsyncRead + Unpin>(stream: &mut S) -> Duration {
    let started = Instant::now();
    let mut buf = [0u8; 1024];
    loop {
        match within("the proxy closing the connection", stream.read(&mut buf)).await {
            Ok(0) | Err(_) => return started.elapsed(),
            Ok(_) => {}
        }
    }
}

/// Upstream sending `one`, then `two` after `gap`: longer than every
/// timeout, but a response in flight must survive it.
async fn slow_stream_upstream(gap: Duration) -> Upstream {
    spawn_upstream(move |req: Request<hyper::body::Incoming>| async move {
        // Uploads may be slow too: read the whole request body first.
        let received = req.into_body().collect().await.map(|b| b.to_bytes().len()).unwrap_or(0);
        let stream = futures::stream::unfold(0u8, move |i| async move {
            match i {
                0 => Some((Ok::<_, BoxError>(Frame::data(Bytes::from(format!("{received}:one")))), 1)),
                1 => {
                    tokio::time::sleep(gap).await;
                    Some((Ok(Frame::data(Bytes::from_static(b":two"))), 2))
                }
                _ => None,
            }
        });
        Response::new(StreamBody::new(stream).boxed_unsync())
    })
    .await
}

/// Websocket echo server.
async fn ws_echo() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
                while let Some(Ok(msg)) = ws.next().await {
                    if (msg.is_text() || msg.is_binary()) && ws.send(msg).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, task)
}

/// Clients that connect and then send nothing, only part of the HTTP/2
/// preface, or part of an HTTP/1 head, are all closed (they used to be kept
/// until shutdown: the protocol detection had no timeout).
#[tokio::test]
async fn connections_without_a_request_are_closed() {
    let proxy = proxy_with(short_timeouts()).await;
    let payloads =
        ["", "P", "PRI * HTTP/2.0\r\n", "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n", "G", "GET / HTTP/1.1\r\nHost: x\r\n"];
    let clients: Vec<_> = payloads
        .into_iter()
        .map(|payload| {
            let addr = proxy.http;
            tokio::spawn(async move {
                let mut stream = TcpStream::connect(addr).await.unwrap();
                stream.write_all(payload.as_bytes()).await.unwrap();
                (payload, closed_after(&mut stream).await)
            })
        })
        .collect();
    for client in clients {
        let (payload, took) = client.await.unwrap();
        assert!(took >= SHORT / 2, "{payload:?} closed too early: {took:?}");
        assert!(took < CLOSED_BY, "{payload:?} closed late: {took:?}");
    }
}

/// Same after a TLS handshake.
#[tokio::test]
async fn tls_connections_without_a_request_are_closed() {
    let (tls, cert) = self_signed(&["secure.test"]);
    let proxy = TestProxy::start_with(Options { tls: Some(tls), limits: short_timeouts(), ..Options::default() }).await;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let tcp = TcpStream::connect(proxy.https.unwrap()).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("secure.test").unwrap();
    let mut tls = within("TLS handshake", connector.connect(name, tcp)).await.unwrap();
    let took = closed_after(&mut tls).await;
    assert!(took < CLOSED_BY, "{took:?}");
}

/// Keep-alive connections are closed once idle, over HTTP/1 and HTTP/2.
#[tokio::test]
async fn idle_keep_alive_connections_are_closed() {
    let up = echo_upstream("a").await;
    let proxy = proxy_with(short_timeouts()).await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let (mut h1, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
    let h1_conn = tokio::spawn(conn);
    for _ in 0..2 {
        let req = Request::get("/").header("host", "app.test").body(Empty::<Bytes>::new()).unwrap();
        let resp = collect(within("h1 request", h1.send_request(req)).await.unwrap()).await;
        assert_eq!(resp.status, 200);
    }
    let idle_from = Instant::now();
    within("idle HTTP/1 connection closing", h1_conn).await.unwrap().unwrap();
    assert!(idle_from.elapsed() >= SHORT / 2, "{:?}", idle_from.elapsed());

    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let (mut h2, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream)).await.unwrap();
    let h2_conn = tokio::spawn(conn);
    for _ in 0..2 {
        let req = Request::get(format!("http://app.test:{}/", proxy.http.port())).body(Empty::<Bytes>::new()).unwrap();
        let resp = collect(within("h2 request", h2.send_request(req)).await.unwrap()).await;
        assert_eq!(resp.status, 200);
    }
    let idle_from = Instant::now();
    within("idle HTTP/2 connection closing", h2_conn).await.unwrap().unwrap();
    assert!(idle_from.elapsed() >= SHORT / 2, "{:?}", idle_from.elapsed());
}

/// Slow uploads, long-lived streamed responses and quiet websockets outlive
/// every timeout: only connections without a request in flight are closed.
#[tokio::test]
async fn requests_in_flight_and_tunnels_outlive_the_timeouts() {
    let gap = SHORT * 3;
    let up = slow_stream_upstream(gap).await;
    let (ws_addr, ws_task) = ws_echo().await;
    let proxy = proxy_with(short_timeouts()).await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    proxy.route("ws", &["ws.test"], &[ws_addr]);

    // HTTP/1: a slow upload, then a slow response.
    let upload = futures::stream::unfold(0u8, move |i| async move {
        match i {
            0 => Some((Ok::<_, std::io::Error>(Bytes::from_static(b"abc")), 1)),
            1 => {
                tokio::time::sleep(gap).await;
                Some((Ok(Bytes::from_static(b"de")), 2))
            }
            _ => None,
        }
    });
    let r = client().post(proxy.url("app.test", "/")).body(reqwest::Body::wrap_stream(upload)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "5:one:two");

    // HTTP/2: a slow response.
    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let (mut h2, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let req = Request::get(format!("http://app.test:{}/", proxy.http.port())).body(Empty::<Bytes>::new()).unwrap();
    let resp = collect(within("h2 request", h2.send_request(req)).await.unwrap()).await;
    assert_eq!(resp.body, "0:one:two");

    // A websocket tunnel quiet for longer than every timeout.
    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let url = format!("ws://ws.test:{}/", proxy.http.port());
    let (mut ws, _) = within("websocket handshake", tokio_tungstenite::client_async(url, stream)).await.unwrap();
    tokio::time::sleep(gap).await;
    ws.send(Message::text("still there")).await.unwrap();
    let msg = within("websocket echo", ws.next()).await.unwrap().unwrap();
    assert_eq!(msg.to_text().unwrap(), "still there");
    ws_task.abort();
}

/// At the connection cap, connections without a request in flight are
/// reclaimed quickly (well before the regular 30 s header timeout), so a
/// real client gets through. Once enough slots are free again, the others
/// are back to the regular timeouts.
#[tokio::test]
async fn connection_cap_reclaims_idle_connections() {
    let up = echo_upstream("a").await;
    let limits =
        ConnectionLimits { max_connections: Some(3), overload_idle_timeout: SHORT, ..ConnectionLimits::default() };
    let proxy = proxy_with(limits).await;
    proxy.route("srv", &["app.test"], &[up.addr]);

    let c = client();
    let mut silent = Vec::new();
    for _ in 0..3 {
        silent.push(TcpStream::connect(proxy.http).await.unwrap());
    }
    let started = Instant::now();
    let r = c.get(proxy.url("app.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    // Silent connections were closed to make room (which ones depends on
    // timing: the rest go back to the regular timeouts once slots are free).
    let closed = futures::future::join_all(silent.iter_mut().map(|stream| async move {
        let mut buf = [0u8; 16];
        matches!(tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await, Ok(Ok(0) | Err(_)))
    }))
    .await;
    assert!(closed.contains(&true), "{closed:?}");
}

/// At the cap, new connections wait (in the listen backlog) for a free
/// slot; websocket tunnels hold theirs until they close.
#[tokio::test]
async fn connection_cap_queues_new_connections() {
    let up = echo_upstream("a").await;
    let (ws_addr, ws_task) = ws_echo().await;
    let limits = ConnectionLimits { max_connections: Some(1), ..ConnectionLimits::default() };
    let proxy = proxy_with(limits).await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    proxy.route("ws", &["ws.test"], &[ws_addr]);

    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let url = format!("ws://ws.test:{}/", proxy.http.port());
    let (mut ws, _) = within("websocket handshake", tokio_tungstenite::client_async(url, stream)).await.unwrap();
    ws.send(Message::text("hi")).await.unwrap();
    assert_eq!(within("websocket echo", ws.next()).await.unwrap().unwrap().to_text().unwrap(), "hi");

    let request = tokio::spawn({
        let url = proxy.url("app.test", "/");
        async move { client().get(url).send().await.map(|r| r.status()) }
    });
    tokio::time::sleep(SHORT * 2).await;
    assert!(!request.is_finished(), "served while the only slot was taken by the websocket");

    drop(ws);
    let status = within("queued request", request).await.unwrap().unwrap();
    assert_eq!(status, 200);
    ws_task.abort();
}

/// `body::with_guard` passes the body through unchanged.
#[tokio::test]
async fn guarded_bodies_are_passed_through() {
    let body = body::with_guard(body::full("payload"), ());
    assert_eq!(body.size_hint().exact(), Some(7));
    assert_eq!(body.collect().await.unwrap().to_bytes(), "payload");
}

/// The cap is shared by both listeners, and a single free slot serves
/// whichever one gets the next client (no listener starves the other).
#[tokio::test]
async fn one_connection_slot_serves_both_listeners() {
    let up = echo_upstream("a").await;
    let (tls, cert) = self_signed(&["app.test"]);
    let limits = ConnectionLimits { max_connections: Some(1), ..ConnectionLimits::default() };
    let proxy = TestProxy::start_with(Options { tls: Some(tls), limits, ..Options::default() }).await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let c =
        client_for(&["app.test"]).add_root_certificate(reqwest::Certificate::from_der(&cert).unwrap()).build().unwrap();
    let https = format!("https://app.test:{}/", proxy.https.unwrap().port());
    for url in
        [proxy.url("app.test", "/"), proxy.url("app.test", "/"), https.clone(), https, proxy.url("app.test", "/")]
    {
        let r = within("request", c.get(&url).header("connection", "close").send()).await.unwrap();
        assert_eq!(r.status(), 200, "{url}");
        assert_eq!(r.headers()["x-upstream"], "a");
    }
}
