//! Black-box tests of the public `serve` entry point.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use ferry_core::{CancellationToken, Error};
use ferry_proxy::{ProxyConfig, RouteTable, serve};

/// A port that was free a moment ago (the listener is dropped before use).
fn free_addr() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

async fn wait_listening(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while tokio::net::TcpStream::connect(addr).await.is_err() {
        assert!(Instant::now() < deadline, "proxy never started listening on {addr}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn returns_bind_errors() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap();
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        serve(ProxyConfig::http(addr), RouteTable::new(), CancellationToken::new()),
    )
    .await
    .expect("serve must fail fast when the port is taken")
    .unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err:?}");
    assert!(err.to_string().contains(&addr.to_string()), "{err}");
}

#[tokio::test]
async fn rejects_https_address_without_tls_config() {
    let mut config = ProxyConfig::http(free_addr());
    config.https_addr = Some(free_addr());
    let err = serve(config, RouteTable::new(), CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
}

#[tokio::test]
async fn serves_until_cancelled() {
    let addr = free_addr();
    let routes = RouteTable::new();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(serve(ProxyConfig::http(addr), routes.clone(), shutdown.clone()));
    wait_listening(addr).await;

    let client =
        reqwest::Client::builder().no_proxy().resolve("nobody.test", "127.0.0.1:0".parse().unwrap()).build().unwrap();
    let r = client.get(format!("http://nobody.test:{}/", addr.port())).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-ferry-error"], "not_found");

    // The route table handed to `serve` is live.
    routes.set_service_routes("srv", &["nobody.test".to_string()], vec![]);
    let r = client.get(format!("http://nobody.test:{}/", addr.port())).send().await.unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["retry-after"], "5");

    let started = Instant::now();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(12), task).await.expect("serve did not stop").unwrap().unwrap();
    // Only an idle keep-alive connection was open: no need to wait for the drain deadline.
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}
