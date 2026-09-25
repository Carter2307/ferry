//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Datelike, Utc};
use ferry_tls::{AcmeClientFactory, CertManager, TlsConfig, instant_acme};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn install_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn config(dir: &Path) -> TlsConfig {
    TlsConfig {
        certs_dir: dir.to_path_buf(),
        acme_email: "ops@example.com".into(),
        staging: false,
        directory_url: Some("https://acme.invalid/directory".into()),
    }
}

/// An ACME client factory that counts calls and always fails (no network).
pub fn offline_client() -> (AcmeClientFactory, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let factory: AcmeClientFactory = Arc::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
        Err(instant_acme::Error::Str("CA offline (test)"))
    });
    (factory, calls)
}

pub async fn offline_manager(dir: &Path) -> (Arc<CertManager>, Arc<AtomicUsize>) {
    install_provider();
    let (factory, calls) = offline_client();
    let manager = CertManager::with_acme_client(config(dir), factory).await.expect("manager");
    (manager, calls)
}

/// A test CA and a leaf certificate for `host` signed by it, as if issued by
/// an ACME CA. Written to `<certs_dir>/<host>/{cert,key}.pem` + meta.json.
/// Returns the leaf DER.
pub fn write_issued(certs_dir: &Path, host: &str, not_before: DateTime<Utc>, not_after: DateTime<Utc>) -> Vec<u8> {
    let ymd = |t: DateTime<Utc>| rcgen::date_time_ymd(t.year(), t.month() as u8, t.day() as u8);
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(rcgen::DnType::CommonName, "Ferry Test CA");
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();

    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec![host.to_string()]).unwrap();
    leaf_params.distinguished_name = rcgen::DistinguishedName::new();
    leaf_params.not_before = ymd(not_before);
    leaf_params.not_after = ymd(not_after);
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();

    let dir = certs_dir.join(host);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cert.pem"), format!("{}{}", leaf.pem(), ca.pem())).unwrap();
    std::fs::write(dir.join("key.pem"), leaf_key.serialize_pem()).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        serde_json::json!({"issued_at": not_before, "not_after": not_after}).to_string(),
    )
    .unwrap();
    leaf.der().to_vec()
}

/// Accepts any server certificate (signatures are still checked).
#[derive(Debug)]
pub struct AcceptAnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// What the client saw during a handshake.
#[derive(Debug)]
pub struct Handshake {
    pub chain: Vec<CertificateDer<'static>>,
    pub alpn: Option<Vec<u8>>,
}

impl Handshake {
    /// DNS SANs of the leaf.
    pub fn leaf_names(&self) -> Vec<String> {
        dns_names(&self.chain[0])
    }
}

pub fn dns_names(der: &[u8]) -> Vec<String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).unwrap();
    cert.subject_alternative_name()
        .unwrap()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(d) => Some(d.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Run a TLS handshake (plus one echo round trip) against `server` over an
/// in-memory pipe. `sni: None` sends no server name.
pub async fn handshake(server: Arc<rustls::ServerConfig>, sni: Option<&str>) -> Handshake {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let acceptor = tokio_rustls::TlsAcceptor::from(server);
    let server_task = tokio::spawn(async move {
        let mut tls = acceptor.accept(server_io).await.expect("server handshake");
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.expect("server read");
        tls.write_all(&buf).await.expect("server write");
        tls.shutdown().await.ok();
    });

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut client = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
        .with_no_client_auth();
    client.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    client.enable_sni = sni.is_some();
    let name = ServerName::try_from(sni.unwrap_or("no-sni.invalid").to_string()).unwrap();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
    let mut tls = connector.connect(name, client_io).await.expect("client handshake");
    tls.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tls.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    let (_, conn) = tls.get_ref();
    let result = Handshake {
        chain: conn.peer_certificates().expect("peer certificates").to_vec(),
        alpn: conn.alpn_protocol().map(<[u8]>::to_vec),
    };
    drop(tls);
    server_task.await.unwrap();
    result
}

pub fn temp_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("certs");
    (tmp, path)
}
