//! Public-API tests of `CertManager`: SNI resolution through real TLS
//! handshakes, persistence across restarts, renewal selection and backoff.
//! No network: the ACME client factory always fails.

mod common;

use std::sync::atomic::Ordering;

use chrono::{Duration, Utc};
use common::*;

#[tokio::test]
async fn self_signed_certificate_per_sni_and_default_without_sni() {
    let (_tmp, dir) = temp_dir();
    let (manager, _) = offline_manager(&dir).await;
    let config = manager.server_config();

    let hs = handshake(config.clone(), Some("App.Example.com")).await;
    assert_eq!(hs.leaf_names(), vec!["app.example.com"]);
    assert_eq!(hs.alpn.as_deref(), Some(&b"h2"[..]));
    assert_eq!(hs.chain.len(), 1);

    let local = handshake(config.clone(), Some("web.localhost")).await;
    assert_eq!(local.leaf_names(), vec!["web.localhost"]);

    let none = handshake(config.clone(), None).await;
    assert_eq!(none.leaf_names(), vec!["ferry"]);

    // Same name → same cached certificate.
    let again = handshake(config, Some("app.example.com")).await;
    assert_eq!(again.chain[0], hs.chain[0]);
    assert!(!manager.hooks().has_certificate("app.example.com"));
}

#[tokio::test]
async fn issued_certificate_is_loaded_and_preferred() {
    let (_tmp, dir) = temp_dir();
    let now = Utc::now();
    let leaf = write_issued(&dir, "shop.example.com", now - Duration::days(1), now + Duration::days(80));
    let (manager, calls) = offline_manager(&dir).await;

    let hooks = manager.hooks();
    assert!(hooks.has_certificate("shop.example.com"));
    assert!(hooks.has_certificate("Shop.Example.com:443"));
    assert!(!hooks.has_certificate("other.example.com"));

    let hs = handshake(manager.server_config(), Some("shop.example.com")).await;
    assert_eq!(hs.chain[0].as_ref(), leaf.as_slice());
    assert_eq!(hs.chain.len(), 2, "full chain is served");
    // Other names still get a self-signed fallback.
    let other = handshake(manager.server_config(), Some("other.example.com")).await;
    assert_eq!(other.leaf_names(), vec!["other.example.com"]);

    // Valid for 80 more days: nothing to do, the CA is never contacted.
    manager.ensure_certificates(&["shop.example.com".to_string()]).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn persisted_certificates_survive_a_restart() {
    let (_tmp, dir) = temp_dir();
    let now = Utc::now();
    write_issued(&dir, "a.example.com", now - Duration::days(1), now + Duration::days(60));
    {
        let (first, _) = offline_manager(&dir).await;
        assert!(first.hooks().has_certificate("a.example.com"));
    }
    let (second, calls) = offline_manager(&dir).await;
    assert!(second.hooks().has_certificate("a.example.com"));
    second.ensure_certificates(&["a.example.com".to_string()]).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn expired_and_expiring_certificates_are_renewed_with_backoff() {
    let (_tmp, dir) = temp_dir();
    let now = Utc::now();
    write_issued(&dir, "expired.example.com", now - Duration::days(120), now - Duration::days(30));
    write_issued(&dir, "soon.example.com", now - Duration::days(70), now + Duration::days(10));
    let (manager, calls) = offline_manager(&dir).await;
    let hooks = manager.hooks();
    assert!(!hooks.has_certificate("expired.example.com"));
    // Still valid (renewal pending), so HTTPS redirects keep working.
    assert!(hooks.has_certificate("soon.example.com"));

    // Both are selected: the (failing) CA is contacted once for the pass.
    let hosts = vec!["expired.example.com".to_string(), "soon.example.com".to_string()];
    manager.ensure_certificates(&hosts).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Failure → backoff: the next pass doesn't retry.
    manager.ensure_certificates(&hosts).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // The old certificates keep being served meanwhile.
    assert!(hooks.has_certificate("soon.example.com"));
    let hs = handshake(manager.server_config(), Some("soon.example.com")).await;
    assert_eq!(hs.chain.len(), 2);
}

#[tokio::test]
async fn corrupt_entries_do_not_prevent_startup() {
    let (_tmp, dir) = temp_dir();
    let now = Utc::now();
    write_issued(&dir, "good.example.com", now - Duration::days(1), now + Duration::days(60));
    let bad = dir.join("bad.example.com");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("cert.pem"), "garbage").unwrap();
    std::fs::write(bad.join("key.pem"), "garbage").unwrap();
    std::fs::create_dir_all(dir.join("accounts")).unwrap();
    std::fs::write(dir.join("README"), "not a host").unwrap();

    let (manager, _) = offline_manager(&dir).await;
    assert!(manager.hooks().has_certificate("good.example.com"));
    assert!(!manager.hooks().has_certificate("bad.example.com"));
    // The corrupt host gets a self-signed certificate instead of a failed handshake.
    let hs = handshake(manager.server_config(), Some("bad.example.com")).await;
    assert_eq!(hs.leaf_names(), vec!["bad.example.com"]);
}

#[tokio::test]
async fn local_hosts_are_never_requested() {
    let (_tmp, dir) = temp_dir();
    let (manager, calls) = offline_manager(&dir).await;
    let hosts: Vec<String> =
        ["web.localhost", "ferry.localhost", "db.internal", "app.test", "127.0.0.1"].map(String::from).to_vec();
    manager.ensure_certificates(&hosts).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(manager.hooks().http01_response("anything"), None);
}

#[tokio::test]
async fn certs_dir_is_created() {
    let (_tmp, dir) = temp_dir();
    let nested = dir.join("deeper");
    let (_manager, _) = offline_manager(&nested).await;
    assert!(nested.is_dir());
}
