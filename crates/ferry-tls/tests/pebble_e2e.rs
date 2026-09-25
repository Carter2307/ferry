//! End-to-end tests against Pebble, Let's Encrypt's test ACME server, in
//! Docker. Gated: they only run with `FERRY_E2E=1` (otherwise they print
//! "skipped" and pass). Every container they start is named
//! `ferrytest-tls-<random>` and removed on drop, even when a test fails.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use common::*;
use ferry_tls::instant_acme::Account;
use ferry_tls::{AcmeClientFactory, CertManager, TlsConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PEBBLE_IMAGE: &str = "ghcr.io/letsencrypt/pebble:latest";

fn e2e_enabled() -> bool {
    if std::env::var("FERRY_E2E").as_deref() == Ok("1") {
        true
    } else {
        println!("skipped: set FERRY_E2E=1 to run the Pebble end-to-end tests");
        false
    }
}

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker").args(args).output().expect("running docker")
}

/// A running Pebble container, removed on drop.
struct Pebble {
    name: String,
    port: u16,
    root_pem: PathBuf,
}

impl Pebble {
    /// Start Pebble with the config file `config`; `extra` are extra
    /// `docker run` arguments.
    async fn start(workdir: &Path, extra: &[&str], config: &Path) -> Pebble {
        let name = format!("ferrytest-tls-{}", ferry_core::ids::random_secret(8));
        let label = format!("ferry.instance={name}");
        let mut args: Vec<String> = ["run", "-d", "--name", &name, "--label", &label, "-p", "127.0.0.1::14000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.extend(["-e", "PEBBLE_VA_NOSLEEP=1", "-e", "PEBBLE_WFE_NONCEREJECT=0"].map(String::from));
        args.extend(extra.iter().map(|s| s.to_string()));
        args.extend(["-v".to_string(), format!("{}:/ferry-pebble.json:ro", config.display())]);
        args.push(PEBBLE_IMAGE.to_string());
        args.extend(["-config", "/ferry-pebble.json"].map(String::from));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = docker(&refs);
        // From here on the guard removes the container whatever happens.
        let mut pebble = Pebble { name: name.clone(), port: 0, root_pem: workdir.join("pebble.minica.pem") };
        assert!(out.status.success(), "docker run failed: {}", String::from_utf8_lossy(&out.stderr));

        let port = docker(&["port", &name, "14000/tcp"]);
        let mapping = String::from_utf8_lossy(&port.stdout).lines().next().unwrap_or_default().to_string();
        pebble.port = mapping.rsplit(':').next().and_then(|p| p.trim().parse().ok()).expect("pebble port");

        let root = pebble.root_pem.display().to_string();
        let cp = docker(&["cp", &format!("{name}:/test/certs/pebble.minica.pem"), &root]);
        assert!(cp.status.success(), "docker cp failed: {}", String::from_utf8_lossy(&cp.stderr));

        // Wait for the ACME listener.
        for _ in 0..100 {
            let logs = docker(&["logs", &name]);
            let text = format!("{}{}", String::from_utf8_lossy(&logs.stdout), String::from_utf8_lossy(&logs.stderr));
            if text.contains("Listening on") && tokio::net::TcpStream::connect(("127.0.0.1", pebble.port)).await.is_ok()
            {
                return pebble;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("pebble did not start: {}", String::from_utf8_lossy(&docker(&["logs", &name]).stderr));
    }

    fn directory_url(&self) -> String {
        format!("https://localhost:{}/dir", self.port)
    }

    fn client(&self) -> AcmeClientFactory {
        let root = self.root_pem.clone();
        Arc::new(move || Account::builder_with_root(&root))
    }
}

impl Drop for Pebble {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", "-v", &self.name]).output();
    }
}

/// Pebble config: its stock settings, but the VA validates HTTP-01 on
/// `http_port`, and a single 90-day profile (the stock config also has a
/// 6-day "shortlived" profile that Pebble may pick, which would make the
/// renewal assertions nondeterministic).
fn write_pebble_config(dir: &Path, http_port: u16) -> PathBuf {
    let path = dir.join("pebble-config.json");
    let config = serde_json::json!({"pebble": {
        "listenAddress": "0.0.0.0:14000",
        "managementListenAddress": "0.0.0.0:15000",
        "certificate": "test/certs/localhost/cert.pem",
        "privateKey": "test/certs/localhost/key.pem",
        "httpPort": http_port,
        "tlsPort": 5001,
        "ocspResponderURL": "",
        "externalAccountBindingRequired": false,
        "domainBlocklist": ["blocked-domain.example"],
        "retryAfter": {"authz": 3, "order": 5},
        "keyAlgorithm": "ecdsa",
        "profiles": {"default": {"description": "90 days", "validityPeriod": 7776000}},
    }});
    std::fs::write(&path, config.to_string()).unwrap();
    path
}

fn tls_config(dir: &Path, pebble: &Pebble) -> TlsConfig {
    TlsConfig {
        certs_dir: dir.to_path_buf(),
        acme_email: "ops@example.com".into(),
        staging: false,
        directory_url: Some(pebble.directory_url()),
    }
}

/// Days until the certificate expires.
fn days_left(der: &[u8]) -> i64 {
    let (_, cert) = x509_parser::parse_x509_certificate(der).unwrap();
    (cert.validity().not_after.timestamp() - Utc::now().timestamp()) / 86_400
}

fn issuer_of(der: &[u8]) -> String {
    let (_, cert) = x509_parser::parse_x509_certificate(der).unwrap();
    cert.issuer().to_string()
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issues_persists_renews_and_serves_pebble_certificates() {
    if !e2e_enabled() {
        return;
    }
    install_provider();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("certs");
    let pebble_config = write_pebble_config(tmp.path(), 5002);
    let pebble = Pebble::start(tmp.path(), &["-e", "PEBBLE_VA_ALWAYS_VALID=1"], &pebble_config).await;
    let config = tls_config(&dir, &pebble);

    let manager = CertManager::with_acme_client(config.clone(), pebble.client()).await.unwrap();
    // Taken before issuance: new certificates must show up without rebuilding it.
    let server_config = manager.server_config();
    let hooks = manager.hooks();
    let public = ["app.example.com", "www.example.org", "api.example.net"];
    let mut hosts: Vec<String> = public.iter().map(|s| s.to_string()).collect();
    hosts.push("web.localhost".into());

    manager.ensure_certificates(&hosts).await;
    for host in public {
        assert!(hooks.has_certificate(host), "no certificate for {host}");
        let hs = handshake(server_config.clone(), Some(host)).await;
        assert_eq!(hs.leaf_names(), vec![host]);
        assert!(hs.chain.len() >= 2, "chain includes the intermediate");
        assert!(issuer_of(&hs.chain[0]).contains("Pebble"), "issuer {}", issuer_of(&hs.chain[0]));
        assert!(days_left(&hs.chain[0]) > 80, "90-day certificate");
        let meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(host).join("meta.json")).unwrap()).unwrap();
        assert!(meta["issued_at"].is_string() && meta["not_after"].is_string(), "{meta}");
        assert_eq!(meta["directory_url"], pebble.directory_url().as_str());
        #[cfg(unix)]
        assert_eq!(mode(&dir.join(host).join("key.pem")), 0o600);
    }
    assert!(!hooks.has_certificate("web.localhost"));
    assert!(!dir.join("web.localhost").exists());
    let account_file = dir.join("accounts").join(format!("localhost_{}_dir.json", pebble.port));
    assert!(account_file.is_file(), "account stored at {}", account_file.display());
    #[cfg(unix)]
    assert_eq!(mode(&account_file), 0o600);

    // Nothing to renew: a second pass orders nothing.
    let cert_before = std::fs::read(dir.join("app.example.com").join("cert.pem")).unwrap();
    manager.ensure_certificates(&hosts).await;
    assert_eq!(std::fs::read(dir.join("app.example.com").join("cert.pem")).unwrap(), cert_before);
    drop(manager);

    // Restart with a certificate that expires in 10 days: it is served until
    // renewed, then replaced, reusing the stored ACME account.
    let account_before = std::fs::read(&account_file).unwrap();
    let now = Utc::now();
    write_issued(&dir, "app.example.com", now - ChronoDuration::days(80), now + ChronoDuration::days(10));
    let manager = CertManager::with_acme_client(config.clone(), pebble.client()).await.unwrap();
    for host in public {
        assert!(manager.hooks().has_certificate(host), "{host} not reloaded");
    }
    let old = handshake(manager.server_config(), Some("app.example.com")).await;
    assert!(issuer_of(&old.chain[0]).contains("Ferry Test CA"));
    manager.ensure_certificates(&hosts).await;
    let renewed = handshake(manager.server_config(), Some("app.example.com")).await;
    assert!(issuer_of(&renewed.chain[0]).contains("Pebble"), "renewed issuer {}", issuer_of(&renewed.chain[0]));
    assert!(days_left(&renewed.chain[0]) > 80);
    assert_eq!(std::fs::read(&account_file).unwrap(), account_before, "account reused");

    // A name the CA refuses: logged + backoff, no certificate, no panic.
    manager.ensure_certificates(&["blocked-domain.example".to_string()]).await;
    assert!(!manager.hooks().has_certificate("blocked-domain.example"));
    assert!(!dir.join("blocked-domain.example").exists());
}

/// Serve `/.well-known/acme-challenge/<token>` from the manager's hooks on
/// `listener`, like the proxy does.
/// Returns a counter of challenges answered with 200.
fn serve_challenges(listener: tokio::net::TcpListener, hooks: Arc<dyn ferry_core::tls::TlsHooks>) -> Arc<AtomicUsize> {
    let answered = Arc::new(AtomicUsize::new(0));
    let counter = answered.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let hooks = hooks.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut len = 0;
                while len < buf.len() {
                    match stream.read(&mut buf[len..]).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => len += n,
                    }
                    if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf[..len]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or_default().to_string();
                let answer = path.strip_prefix("/.well-known/acme-challenge/").and_then(|t| hooks.http01_response(t));
                let response = match answer {
                    Some(body) => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    answered
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_http01_validation_through_the_hooks() {
    if !e2e_enabled() {
        return;
    }
    install_provider();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("certs");

    // Pebble's VA connects to <host>:<httpPort>; point the host at the Docker
    // host and the port at our challenge server.
    // Docker Desktop (macOS) forwards host-gateway traffic to the host's
    // loopback; on Linux it arrives on the bridge interface.
    let bind = if cfg!(target_os = "macos") { "127.0.0.1:0" } else { "0.0.0.0:0" };
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let http_port = listener.local_addr().unwrap().port();
    let pebble_config = write_pebble_config(tmp.path(), http_port);
    let pebble = Pebble::start(tmp.path(), &["--add-host", "validate.example.com:host-gateway"], &pebble_config).await;

    let manager = CertManager::with_acme_client(tls_config(&dir, &pebble), pebble.client()).await.unwrap();
    let answered = serve_challenges(listener, manager.hooks());
    manager.ensure_certificates(&["validate.example.com".to_string()]).await;
    assert!(
        manager.hooks().has_certificate("validate.example.com"),
        "HTTP-01 validation failed; pebble logs:\n{}",
        String::from_utf8_lossy(&docker(&["logs", &pebble.name]).stdout)
    );
    assert!(answered.load(Ordering::SeqCst) >= 1, "the CA never fetched the challenge");
    // The challenge is withdrawn once the order is done.
    assert!(manager.hooks().http01_response("anything").is_none());
    let hs = handshake(manager.server_config(), Some("validate.example.com")).await;
    assert_eq!(hs.leaf_names(), vec!["validate.example.com"]);
}
