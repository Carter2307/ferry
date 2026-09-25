//! Unit tests of the manager (no network: the ACME client factory fails).

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Duration as ChronoDuration;

use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    manager: Arc<CertManager>,
    client_calls: Arc<AtomicUsize>,
}

fn offline_client(calls: Arc<AtomicUsize>) -> AcmeClientFactory {
    Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(instant_acme::Error::Str("CA offline (test)"))
    })
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = CertManager::with_acme_client(config(dir.path()), offline_client(calls.clone())).await.unwrap();
    Fixture { _dir: dir, manager, client_calls: calls }
}

fn config(dir: &std::path::Path) -> TlsConfig {
    TlsConfig {
        certs_dir: dir.to_path_buf(),
        acme_email: "ops@example.com".into(),
        staging: false,
        directory_url: Some("https://acme.invalid/directory".into()),
    }
}

fn issued(host: &str, days_left: i64) -> certs::IssuedCert {
    let now = Utc::now();
    let (cert, key) =
        certs::self_signed_pem(host, now - ChronoDuration::days(60), now + ChronoDuration::days(days_left)).unwrap();
    certs::issued_from_pem(cert.as_bytes(), key.as_bytes(), None, &crypto_provider()).unwrap()
}

fn hosts(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn candidate_names(m: &CertManager, list: &[&str]) -> Vec<String> {
    m.select_candidates(&hosts(list), Utc::now()).into_iter().map(|c| c.host).collect()
}

#[test]
fn directory_url_selection() {
    let mut c = config(std::path::Path::new("/x"));
    assert_eq!(c.effective_directory_url(), "https://acme.invalid/directory");
    c.directory_url = Some("  ".into());
    assert_eq!(c.effective_directory_url(), LETS_ENCRYPT_PRODUCTION);
    c.staging = true;
    assert_eq!(c.effective_directory_url(), LETS_ENCRYPT_STAGING);
    c.directory_url = None;
    assert_eq!(c.effective_directory_url(), LETS_ENCRYPT_STAGING);
}

#[test]
fn port_stripping() {
    assert_eq!(strip_port("app.example.com:443"), "app.example.com");
    assert_eq!(strip_port("app.example.com"), "app.example.com");
    assert_eq!(strip_port("[::1]:8443"), "::1");
    assert_eq!(strip_port("::1"), "::1");
    assert_eq!(strip_port("host:notaport"), "host:notaport");
}

#[tokio::test]
async fn http01_response_serves_pending_challenges_only() {
    let f = fixture().await;
    let hooks = f.manager.hooks();
    assert_eq!(hooks.http01_response("tok"), None);
    {
        let mut published = acme::PublishedChallenges::new(&f.manager.challenges);
        published.publish("tok".into(), "tok.thumbprint".into());
        assert_eq!(hooks.http01_response("tok").as_deref(), Some("tok.thumbprint"));
        assert_eq!(hooks.http01_response("other"), None);
    }
    assert_eq!(hooks.http01_response("tok"), None);
}

#[tokio::test]
async fn has_certificate_only_for_unexpired_issued_certs() {
    let f = fixture().await;
    let hooks = f.manager.hooks();
    assert!(!hooks.has_certificate("app.example.com"));
    // a self-signed fallback doesn't count
    f.manager.store.resolve_name(Some("app.example.com")).unwrap();
    assert!(!hooks.has_certificate("app.example.com"));
    f.manager.store.insert_issued("app.example.com", issued("app.example.com", 50));
    assert!(hooks.has_certificate("app.example.com"));
    assert!(hooks.has_certificate("APP.example.com:443"));
    assert!(hooks.has_certificate("app.example.com."));
    f.manager.store.insert_issued("old.example.com", issued("old.example.com", -1));
    assert!(!hooks.has_certificate("old.example.com"));
}

#[tokio::test]
async fn candidate_selection() {
    let f = fixture().await;
    let m = &f.manager;
    m.store.insert_issued("fresh.example.com", issued("fresh.example.com", 60));
    m.store.insert_issued("expiring.example.com", issued("expiring.example.com", 10));
    m.store.insert_issued("expired.example.com", issued("expired.example.com", -5));
    let got = candidate_names(
        m,
        &[
            "web.localhost",
            "ferry.localhost",
            "localhost",
            "127.0.0.1",
            "app.test",
            "svc.internal",
            "printer.local",
            "not a host",
            "*.example.com",
            "nodot",
            "",
            "fresh.example.com",
            "expiring.example.com",
            "Expired.Example.com.",
            "new.example.com",
            "new.example.com",
        ],
    );
    assert_eq!(got, vec!["expiring.example.com", "expired.example.com", "new.example.com"]);
}

#[tokio::test]
async fn certificates_from_another_directory_are_replaced() {
    let f = fixture().await;
    let m = &f.manager;
    let mut staging = issued("staging.example.com", 80);
    staging.directory_url = Some(LETS_ENCRYPT_STAGING.into());
    m.store.insert_issued("staging.example.com", staging);
    let mut same = issued("same.example.com", 80);
    same.directory_url = Some(m.directory_url.clone());
    m.store.insert_issued("same.example.com", same);
    m.store.insert_issued("unknown.example.com", issued("unknown.example.com", 80));
    let got = candidate_names(m, &["staging.example.com", "same.example.com", "unknown.example.com"]);
    assert_eq!(got, vec!["staging.example.com"]);
    // still served (and redirected to) until replaced
    assert!(m.hooks().has_certificate("staging.example.com"));
}

#[tokio::test]
async fn in_flight_hosts_are_not_selected_twice() {
    let f = fixture().await;
    let first = f.manager.select_candidates(&hosts(&["a.example.com"]), Utc::now());
    assert_eq!(first.len(), 1);
    assert!(candidate_names(&f.manager, &["a.example.com"]).is_empty());
    drop(first);
    assert_eq!(candidate_names(&f.manager, &["a.example.com"]), vec!["a.example.com"]);
}

#[tokio::test]
async fn local_hosts_never_contact_the_ca() {
    let f = fixture().await;
    f.manager.ensure_certificates(&hosts(&["web.localhost", "ferry.localhost", "10.0.0.1"])).await;
    f.manager.ensure_certificates(&[]).await;
    assert_eq!(f.client_calls.load(Ordering::SeqCst), 0);
    assert!(f.manager.backoff.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failures_back_off_per_host() {
    let f = fixture().await;
    let m = &f.manager;
    m.ensure_certificates(&hosts(&["a.example.com", "b.example.com"])).await;
    assert_eq!(f.client_calls.load(Ordering::SeqCst), 1);
    {
        let backoff = m.backoff.lock().unwrap();
        assert_eq!(backoff.len(), 2);
        let a = &backoff["a.example.com"];
        assert_eq!(a.failures, 1);
        assert!(a.last_error.contains("CA offline"), "{}", a.last_error);
        let wait = a.next_attempt - tokio::time::Instant::now();
        assert!(wait > backoff::BACKOFF_INITIAL - Duration::from_secs(5) && wait <= backoff::BACKOFF_INITIAL);
    }
    // Retried later only: a second pass doesn't touch the CA.
    m.ensure_certificates(&hosts(&["a.example.com", "b.example.com"])).await;
    assert_eq!(f.client_calls.load(Ordering::SeqCst), 1);
    // A new host is not blocked by the others' backoff.
    m.ensure_certificates(&hosts(&["a.example.com", "c.example.com"])).await;
    assert_eq!(f.client_calls.load(Ordering::SeqCst), 2);
    // Once the backoff elapses, the host is retried and its delay doubles.
    m.backoff.lock().unwrap().get_mut("a.example.com").unwrap().next_attempt = tokio::time::Instant::now();
    m.ensure_certificates(&hosts(&["a.example.com"])).await;
    assert_eq!(f.client_calls.load(Ordering::SeqCst), 3);
    let backoff = m.backoff.lock().unwrap();
    assert_eq!(backoff["a.example.com"].failures, 2);
    let wait = backoff["a.example.com"].next_attempt - tokio::time::Instant::now();
    assert!(wait > backoff::BACKOFF_INITIAL * 2 - Duration::from_secs(5));
    assert!(m.in_flight.lock().unwrap().is_empty());
}

#[tokio::test]
async fn server_config_is_shared_and_negotiates_alpn() {
    let f = fixture().await;
    let a = f.manager.server_config();
    let b = f.manager.server_config();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(a.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
}

#[tokio::test(start_paused = true)]
async fn spawn_runs_immediately_then_every_minute_until_shutdown() {
    let f = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let shutdown = CancellationToken::new();
    let handle = f.manager.spawn(
        Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
            vec!["web.localhost".to_string()]
        }),
        shutdown.clone(),
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::sleep(CHECK_INTERVAL).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    tokio::time::sleep(CHECK_INTERVAL * 2).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    shutdown.cancel();
    handle.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}
