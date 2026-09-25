//! HTTPS termination and HTTP → HTTPS redirects.

use std::collections::HashSet;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::support::*;

async fn tls_proxy(redirect_https: bool) -> (TestProxy, rustls::pki_types::CertificateDer<'static>, Upstream) {
    let (tls, cert) = self_signed(&["secure.test", "plain.test", "unknown.test"]);
    let hooks = MockHooks { certificates: HashSet::from(["secure.test".to_string()]), ..MockHooks::default() };
    let proxy = TestProxy::start_with(Options {
        tls: Some(tls),
        hooks: Some(Arc::new(hooks)),
        redirect_https,
        ..Options::default()
    })
    .await;
    let up = echo_upstream("tls-app").await;
    proxy.route("secure", &["secure.test"], &[up.addr]);
    proxy.route("plain", &["plain.test"], &[up.addr]);
    (proxy, cert, up)
}

fn tls_client(cert: &rustls::pki_types::CertificateDer<'static>) -> reqwest::Client {
    client_for(&["secure.test", "plain.test", "unknown.test"])
        .add_root_certificate(reqwest::Certificate::from_der(cert).unwrap())
        .build()
        .unwrap()
}

#[tokio::test]
async fn terminates_https_and_forwards_as_http() {
    let (proxy, cert, _up) = tls_proxy(true).await;
    let https_port = proxy.https.unwrap().port();
    let c = tls_client(&cert);

    let r = c.get(format!("https://secure.test:{https_port}/p?q=1")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let echo = Echo::parse(&r.text().await.unwrap());
    assert_eq!(echo.path, "/p?q=1");
    assert_eq!(echo.header("host"), Some(format!("secure.test:{https_port}").as_str()));
    assert_eq!(echo.header("x-forwarded-proto"), Some("https"));
    assert_eq!(echo.header("x-forwarded-port"), Some(https_port.to_string().as_str()));

    // ACME challenges are only answered on plain HTTP; on HTTPS the app gets them.
    let r = c.get(format!("https://secure.test:{https_port}/.well-known/acme-challenge/x")).send().await.unwrap();
    assert_eq!(r.headers()["x-upstream"], "tls-app");

    // Hosts without a CA certificate are still served over HTTPS (the
    // resolver decides which certificate is presented).
    let r = c.get(format!("https://plain.test:{https_port}/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(Echo::parse(&r.text().await.unwrap()).header("x-forwarded-proto"), Some("https"));

    // Unknown hosts get the error page over TLS too.
    let r = c.get(format!("https://unknown.test:{https_port}/")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-ferry-error"], "not_found");
    assert!(r.text().await.unwrap().contains("No service is configured for unknown.test"));
}

#[tokio::test]
async fn redirects_http_to_https_when_host_has_certificate() {
    let (proxy, cert, _up) = tls_proxy(true).await;
    let https_port = proxy.https.unwrap().port();
    let c = tls_client(&cert);

    let r = c.get(proxy.url("secure.test", "/p/a%20b?q=1&r=2")).send().await.unwrap();
    assert_eq!(r.status(), 308);
    assert_eq!(r.headers()["location"], format!("https://secure.test:{https_port}/p/a%20b?q=1&r=2"));

    // Host matching for the certificate check is normalized.
    let resp =
        h1_send(proxy.http, http::Request::post("/x").header("host", "SECURE.test.").body(Full::from("body")).unwrap())
            .await;
    assert_eq!(resp.status, 308);
    assert_eq!(resp.headers["location"], format!("https://secure.test:{https_port}/x"));

    // No certificate → served over plain HTTP.
    let r = c.get(proxy.url("plain.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-upstream"], "tls-app");

    // Following the redirect works end to end.
    let follow = client_for(&["secure.test"])
        .add_root_certificate(reqwest::Certificate::from_der(&cert).unwrap())
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .unwrap();
    let r = follow.get(proxy.url("secure.test", "/followed")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.url().scheme(), "https");
    assert_eq!(Echo::parse(&r.text().await.unwrap()).path, "/followed");
}

#[tokio::test]
async fn no_redirect_when_disabled() {
    let (proxy, cert, _up) = tls_proxy(false).await;
    let r = tls_client(&cert).get(proxy.url("secure.test", "/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-upstream"], "tls-app");
}

#[tokio::test]
async fn http2_over_tls_with_alpn() {
    let (proxy, cert, _up) = tls_proxy(true).await;
    let https = proxy.https.unwrap();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let mut config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(https).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("secure.test").unwrap();
    let tls = within("TLS handshake", connector.connect(name, tcp)).await.unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));

    let (mut sender, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls)).await.unwrap();
    tokio::spawn(conn);
    let req = http::Request::get(format!("https://secure.test:{}/over-h2", https.port()))
        .body(Full::<Bytes>::default())
        .unwrap();
    let resp = collect(within("h2 request", sender.send_request(req)).await.unwrap()).await;
    assert_eq!(resp.status, 200);
    let echo = Echo::parse(&resp.body);
    assert_eq!(echo.version, "HTTP/1.1");
    assert_eq!(echo.path, "/over-h2");
    assert_eq!(echo.header("host"), Some(format!("secure.test:{}", https.port()).as_str()));
    assert_eq!(echo.header("x-forwarded-proto"), Some("https"));
}

#[tokio::test]
async fn failed_handshakes_do_not_affect_the_listener() {
    let (proxy, cert, _up) = tls_proxy(true).await;
    let https = proxy.https.unwrap();

    // Plain HTTP spoken to the TLS port.
    let mut s = TcpStream::connect(https).await.unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: secure.test\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    let _ = within("rejected handshake", s.read_to_end(&mut buf)).await;

    // A client that doesn't trust the certificate.
    let untrusting = client_for(&["secure.test"]).build().unwrap();
    assert!(untrusting.get(format!("https://secure.test:{}/", https.port())).send().await.is_err());

    let r = tls_client(&cert).get(format!("https://secure.test:{}/", https.port())).send().await.unwrap();
    assert_eq!(r.status(), 200);
}
