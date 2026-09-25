//! Graceful shutdown: stop accepting, drain in-flight requests, bounded wait.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use http::Response;
use tokio::net::TcpStream;
use tokio::sync::Notify;

use super::support::*;
use crate::body::{self, ProxyBody};

#[tokio::test]
async fn in_flight_requests_finish_and_new_connections_are_refused() {
    let arrived = Arc::new(Notify::new());
    let up = spawn_upstream({
        let arrived = arrived.clone();
        move |_req| {
            let arrived = arrived.clone();
            async move {
                arrived.notify_one();
                tokio::time::sleep(Duration::from_millis(400)).await;
                Response::new(body::full("slow done"))
            }
        }
    })
    .await;
    let mut proxy = TestProxy::start().await;
    proxy.route("srv", &["slow.test"], &[up.addr]);

    let c = client_for(&["slow.test"]).build().unwrap();
    let url = proxy.url("slow.test", "/");
    let request = tokio::spawn(async move { c.get(url).send().await?.text().await });
    within("request reaching upstream", arrived.notified()).await;

    let took = proxy.stop().await;
    assert_eq!(within("in-flight response", request).await.unwrap().unwrap(), "slow done");
    assert!(took >= Duration::from_millis(200), "returned before the in-flight request finished: {took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(TcpStream::connect(proxy.http).await.is_err(), "listener still accepting after shutdown");
}

#[tokio::test]
async fn hung_requests_are_cut_at_the_drain_deadline() {
    let arrived = Arc::new(Notify::new());
    let up = spawn_upstream({
        let arrived = arrived.clone();
        move |_req| {
            let arrived = arrived.clone();
            async move {
                arrived.notify_one();
                std::future::pending::<Response<ProxyBody>>().await
            }
        }
    })
    .await;
    let mut proxy = TestProxy::start_with(Options { drain: Duration::from_millis(500), ..Options::default() }).await;
    proxy.route("srv", &["hang.test"], &[up.addr]);

    let c = client_for(&["hang.test"]).build().unwrap();
    let url = proxy.url("hang.test", "/");
    let request = tokio::spawn(async move { c.get(url).send().await });
    within("request reaching upstream", arrived.notified()).await;

    let took = proxy.stop().await;
    assert!(took >= Duration::from_millis(450), "{took:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
    assert!(within("client error", request).await.unwrap().is_err());
}

#[tokio::test]
async fn idle_keep_alive_connections_do_not_delay_shutdown() {
    let up = echo_upstream("a").await;
    // Default 10 s drain: an idle connection must not hold it up.
    let mut proxy = TestProxy::start().await;
    proxy.route("srv", &["app.test"], &[up.addr]);
    let c = client();
    for _ in 0..3 {
        assert_eq!(c.get(proxy.url("app.test", "/")).send().await.unwrap().status(), 200);
    }
    // Also an HTTP/2 connection kept open.
    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let (_sender, conn) = hyper::client::conn::http2::handshake::<_, _, http_body_util::Empty<bytes::Bytes>>(
        hyper_util::rt::TokioExecutor::new(),
        hyper_util::rt::TokioIo::new(stream),
    )
    .await
    .unwrap();
    let h2 = tokio::spawn(conn);

    let took = proxy.stop().await;
    assert!(took < Duration::from_secs(2), "{took:?}");
    let _ = within("h2 connection closing", h2).await;
    drop(c);
}

#[tokio::test]
async fn websocket_tunnels_close_on_shutdown() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    while let Some(Ok(_)) = ws.next().await {}
                }
            });
        }
    });
    let mut proxy = TestProxy::start().await;
    proxy.route("ws", &["ws.test"], &[addr]);
    let stream = TcpStream::connect(proxy.http).await.unwrap();
    let url = format!("ws://ws.test:{}/", proxy.http.port());
    let (mut ws, _) = within("handshake", tokio_tungstenite::client_async(url, stream)).await.unwrap();

    let took = proxy.stop().await;
    assert!(took < Duration::from_secs(2), "{took:?}");
    // The client sees the connection end.
    let next = within("tunnel closing", ws.next()).await;
    assert!(!matches!(next, Some(Ok(ref m)) if m.is_text() || m.is_binary()), "{next:?}");
    upstream.abort();
}

#[tokio::test]
async fn https_listener_without_tls_config_is_rejected() {
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let https = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = crate::ProxyConfig::http(http.local_addr().unwrap());
    let err = crate::server::serve_on(
        &config,
        http,
        Some(https),
        crate::RouteTable::new(),
        ferry_core::CancellationToken::new(),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ferry_core::Error::Invalid(_)), "{err}");
}
