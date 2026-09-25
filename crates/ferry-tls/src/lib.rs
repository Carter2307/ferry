//! # ferry-tls
//!
//! Automatic HTTPS for custom domains:
//! * obtains certificates from an ACME CA (Let's Encrypt by default) with the
//!   HTTP-01 challenge — the proxy answers `/.well-known/acme-challenge/*`
//!   through [`TlsHooks::http01_response`];
//! * stores account + certificates under `certs_dir` (PEM, 0600 keys) and
//!   reloads them on start;
//! * renews certificates expiring in < 30 days;
//! * resolves certificates by SNI for the proxy's rustls `ServerConfig`,
//!   falling back to a cached self-signed certificate (rcgen) for hosts
//!   without one (e.g. `*.localhost`) so TLS handshakes never hard-fail.
//!
//! Local hosts (`ferry_core::config::is_local_host`) are never requested
//! from the CA.

use std::path::PathBuf;
use std::sync::Arc;

use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Result};

/// Let's Encrypt production directory.
pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// Let's Encrypt staging directory (untrusted certs, generous rate limits).
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Certificate manager settings.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Where the ACME account and certificates live.
    pub certs_dir: PathBuf,
    /// Contact email for the ACME account.
    pub acme_email: String,
    /// Use the staging directory.
    pub staging: bool,
    /// Override the directory URL (e.g. a local Pebble for tests).
    pub directory_url: Option<String>,
}

/// Obtains, stores, renews and serves certificates.
pub struct CertManager {
    config: TlsConfig,
}

impl std::fmt::Debug for CertManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertManager").field("certs_dir", &self.config.certs_dir).finish_non_exhaustive()
    }
}

impl CertManager {
    /// Create the manager and load existing certificates from disk. Does not
    /// contact the CA.
    pub async fn new(config: TlsConfig) -> Result<Arc<Self>> {
        let _ = config;
        todo!("ferry-tls: new")
    }

    /// rustls server config (ALPN h2 + http/1.1) whose cert resolver picks the
    /// certificate by SNI (self-signed fallback). Reflects certificates
    /// obtained later without rebuilding.
    pub fn server_config(self: &Arc<Self>) -> Arc<rustls::ServerConfig> {
        todo!("ferry-tls: server_config")
    }

    /// Hooks for the proxy (challenge answers, has-certificate checks).
    pub fn hooks(self: &Arc<Self>) -> Arc<dyn TlsHooks> {
        todo!("ferry-tls: hooks")
    }

    /// Obtain certificates for hosts that lack one and renew those expiring
    /// within 30 days. Local hosts are skipped. Failures are logged and
    /// retried on a later call (with per-host backoff); never panics.
    pub async fn ensure_certificates(self: &Arc<Self>, hosts: &[String]) {
        let _ = hosts;
        todo!("ferry-tls: ensure_certificates")
    }

    /// Background loop: every 60 s, call `ensure_certificates(hosts())`.
    pub fn spawn(
        self: &Arc<Self>,
        hosts: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let _ = (hosts, shutdown);
        todo!("ferry-tls: spawn")
    }
}
