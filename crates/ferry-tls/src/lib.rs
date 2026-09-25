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
//!
//! ## Disk layout
//!
//! ```text
//! certs_dir/
//!   accounts/<sanitized-directory-url>.json   ACME account (0600), one per CA directory
//!   <host>/cert.pem                           full chain, leaf first
//!   <host>/key.pem                            ECDSA P-256 private key (0600)
//!   <host>/meta.json                          {"issued_at": …, "not_after": …}
//! ```

mod acme;
mod backoff;
mod certs;
mod resolver;
mod storage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ferry_core::config::is_local_host;
use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Error, Result};
use instant_acme::Account;
use rustls::crypto::CryptoProvider;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::acme::IssueError;
use crate::backoff::Backoff;
use crate::resolver::{CertStore, IssuedSummary, SniResolver, normalize_host};

/// Re-export, so callers can build a custom [`AcmeClientFactory`] without
/// depending on `instant-acme` themselves.
pub use instant_acme;

/// Let's Encrypt production directory.
pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// Let's Encrypt staging directory (untrusted certs, generous rate limits).
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Certificates expiring within this many days are renewed.
pub const RENEW_BEFORE_DAYS: i64 = 30;
/// Interval of the background loop started by [`CertManager::spawn`].
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Maximum number of ACME orders in flight at the same time.
pub const MAX_CONCURRENT_ORDERS: usize = 2;
/// Budget for restoring/registering the ACME account.
const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(90);
/// Budget for one complete order (challenge validation + issuance).
const ORDER_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Builds the ACME client each time the manager restores or registers its
/// ACME account. The default is [`instant_acme::Account::builder`] (HTTPS with
/// the platform's trust store); tests and private CAs can use e.g.
/// `Arc::new(move || Account::builder_with_root(&ca_pem_path))`.
pub type AcmeClientFactory =
    Arc<dyn Fn() -> std::result::Result<instant_acme::AccountBuilder, instant_acme::Error> + Send + Sync>;

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

impl TlsConfig {
    /// The ACME directory in use: the override, else Let's Encrypt
    /// staging/production.
    pub fn effective_directory_url(&self) -> &str {
        match self.directory_url.as_deref().map(str::trim) {
            Some(url) if !url.is_empty() => url,
            _ if self.staging => LETS_ENCRYPT_STAGING,
            _ => LETS_ENCRYPT_PRODUCTION,
        }
    }
}

/// Obtains, stores, renews and serves certificates.
pub struct CertManager {
    config: TlsConfig,
    directory_url: String,
    store: Arc<CertStore>,
    server_config: OnceLock<Arc<rustls::ServerConfig>>,
    /// Pending HTTP-01 challenges: token → key authorization.
    challenges: RwLock<HashMap<String, String>>,
    backoff: Mutex<HashMap<String, Backoff>>,
    /// Hosts with an order in progress (overlapping `ensure_certificates`
    /// calls never order the same host twice).
    in_flight: Mutex<HashSet<String>>,
    order_slots: Semaphore,
    account: tokio::sync::Mutex<Option<Account>>,
    acme_client: AcmeClientFactory,
}

impl std::fmt::Debug for CertManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertManager")
            .field("certs_dir", &self.config.certs_dir)
            .field("directory_url", &self.directory_url)
            .field("certificates", &self.store.issued_count())
            .finish_non_exhaustive()
    }
}

/// The process-wide rustls crypto provider, installing ring when none is set:
/// instant-acme's HTTPS client builds its rustls config from the process
/// default and would panic without one.
fn crypto_provider() -> Arc<CryptoProvider> {
    if CryptoProvider::get_default().is_none() {
        // Losing an install race to another thread is fine: a default exists either way.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    CryptoProvider::get_default().cloned().unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()))
}

/// Strip a `:port` suffix (not from bare IPv6 addresses).
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.parse::<u16>().is_ok() => h,
        _ => host,
    }
}

/// Removes its host from the in-flight set when dropped.
struct InFlight<'a> {
    set: &'a Mutex<HashSet<String>>,
    host: String,
}

impl<'a> InFlight<'a> {
    fn acquire(set: &'a Mutex<HashSet<String>>, host: &str) -> Option<Self> {
        let inserted = set.lock().unwrap_or_else(PoisonError::into_inner).insert(host.to_string());
        inserted.then(|| InFlight { set, host: host.to_string() })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.set.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.host);
    }
}

/// A host selected for issuance/renewal.
struct Candidate<'a> {
    host: String,
    /// The certificate being renewed or replaced.
    current: Option<IssuedSummary>,
    _in_flight: InFlight<'a>,
}

impl CertManager {
    /// Create the manager and load existing certificates from disk. Does not
    /// contact the CA.
    pub async fn new(config: TlsConfig) -> Result<Arc<Self>> {
        Self::with_acme_client(config, Arc::new(Account::builder)).await
    }

    /// Like [`CertManager::new`], but reaches the ACME CA through clients
    /// built by `acme_client` (e.g. to trust a private or test CA).
    pub async fn with_acme_client(config: TlsConfig, acme_client: AcmeClientFactory) -> Result<Arc<Self>> {
        let provider = crypto_provider();
        let certs_dir = config.certs_dir.clone();
        let store = tokio::task::spawn_blocking(move || -> Result<CertStore> {
            storage::create_private_dir(&certs_dir)
                .map_err(|e| Error::internal(format!("creating certificate directory {}: {e}", certs_dir.display())))?;
            let store = CertStore::new(provider.clone(), resolver::SELF_SIGNED_CACHE_CAPACITY)?;
            for (host, cert) in storage::load_all(&certs_dir, &provider) {
                debug!("loaded certificate for {host} (valid until {})", cert.not_after);
                store.insert_issued(&host, cert);
            }
            Ok(store)
        })
        .await
        .map_err(|e| Error::internal(format!("loading certificates: {e}")))??;

        let directory_url = config.effective_directory_url().to_string();
        info!(
            "TLS: {} certificate(s) loaded from {}; ACME directory {directory_url}",
            store.issued_count(),
            config.certs_dir.display()
        );
        Ok(Arc::new(CertManager {
            directory_url,
            config,
            store: Arc::new(store),
            server_config: OnceLock::new(),
            challenges: RwLock::new(HashMap::new()),
            backoff: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashSet::new()),
            order_slots: Semaphore::new(MAX_CONCURRENT_ORDERS),
            account: tokio::sync::Mutex::new(None),
            acme_client,
        }))
    }

    /// rustls server config (ALPN h2 + http/1.1) whose cert resolver picks the
    /// certificate by SNI (self-signed fallback). Reflects certificates
    /// obtained later without rebuilding.
    pub fn server_config(self: &Arc<Self>) -> Arc<rustls::ServerConfig> {
        self.server_config.get_or_init(|| build_server_config(&self.store)).clone()
    }

    /// Hooks for the proxy (challenge answers, has-certificate checks).
    pub fn hooks(self: &Arc<Self>) -> Arc<dyn TlsHooks> {
        self.clone()
    }

    /// Obtain certificates for hosts that lack one and renew those expiring
    /// within 30 days. Local hosts are skipped. Failures are logged and
    /// retried on a later call (with per-host backoff); never panics.
    ///
    /// Certificates with a lifetime under 90 days are renewed when the last
    /// third of their lifetime starts instead (e.g. 2 days before expiry for
    /// a 6-day certificate), so short-lived certificates aren't re-ordered on
    /// every pass. At most [`MAX_CONCURRENT_ORDERS`] orders run at once.
    pub async fn ensure_certificates(self: &Arc<Self>, hosts: &[String]) {
        let candidates = self.select_candidates(hosts, Utc::now());
        if candidates.is_empty() {
            return;
        }
        let account = match self.account().await {
            Ok(account) => account,
            Err(e) => {
                let hosts: Vec<&str> = candidates.iter().map(|c| c.host.as_str()).collect();
                warn!(
                    "ACME account at {} unavailable: {e}; postponing certificates for {}",
                    self.directory_url,
                    hosts.join(", ")
                );
                for c in &candidates {
                    self.record_failure(&c.host, &e.message, false);
                }
                return;
            }
        };
        futures::future::join_all(candidates.into_iter().map(|c| self.process(&account, c))).await;
    }

    /// Background loop: every 60 s, call `ensure_certificates(hosts())`.
    pub fn spawn(
        self: &Arc<Self>,
        hosts: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        tokio::spawn(async move {
            debug!("certificate manager loop started");
            loop {
                let list = hosts();
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = manager.ensure_certificates(&list) => {}
                }
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(CHECK_INTERVAL) => {}
                }
            }
            debug!("certificate manager loop stopped");
        })
    }

    /// Hosts that need a certificate now (see [`CertManager::ensure_certificates`]).
    fn select_candidates(&self, hosts: &[String], now: DateTime<Utc>) -> Vec<Candidate<'_>> {
        let clock = tokio::time::Instant::now();
        let mut backoff = self.backoff.lock().unwrap_or_else(PoisonError::into_inner);
        backoff.retain(|_, b| !b.stale(clock));
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for raw in hosts {
            let host = normalize_host(raw);
            if host.is_empty() || is_local_host(&host) || !seen.insert(host.clone()) {
                continue;
            }
            match ferry_core::validate::domain(&host) {
                Ok(valid) if valid == host => {}
                _ => {
                    debug!("not requesting a certificate for '{raw}': not a valid domain name");
                    continue;
                }
            }
            let current = self.store.issued_summary(&host);
            if let Some(c) = &current
                && now < c.renew_after
                && !self.issued_elsewhere(c)
            {
                continue;
            }
            if let Some(b) = backoff.get(&host)
                && b.blocks(clock)
            {
                debug!("{host}: next certificate attempt after backoff (last error: {})", b.last_error);
                continue;
            }
            let Some(in_flight) = InFlight::acquire(&self.in_flight, &host) else {
                continue;
            };
            out.push(Candidate { host, current, _in_flight: in_flight });
        }
        out
    }

    /// Order, persist and install the certificate of one candidate.
    async fn process(&self, account: &Account, candidate: Candidate<'_>) {
        let host = candidate.host.as_str();
        let Ok(_slot) = self.order_slots.acquire().await else {
            return; // the semaphore is never closed
        };
        match &candidate.current {
            None => info!("requesting a TLS certificate for {host}"),
            Some(c) if self.issued_elsewhere(c) => info!(
                "replacing the TLS certificate for {host}: issued by {}, but {} is configured",
                c.directory_url.as_deref().unwrap_or("?"),
                self.directory_url
            ),
            Some(c) => info!("renewing the TLS certificate for {host} (expires {})", c.not_after),
        }
        let result = match tokio::time::timeout(ORDER_TIMEOUT, self.obtain(account, host)).await {
            Ok(result) => result,
            Err(_) => Err(IssueError::other(format!("gave up after {}s", ORDER_TIMEOUT.as_secs()))),
        };
        match result {
            Ok((renew_after, not_after)) if renew_after <= Utc::now() => {
                // Installed, but so short-lived that it's already due: back
                // off instead of re-ordering on every pass.
                let msg = format!("the CA issued a certificate that is already due for renewal (expires {not_after})");
                self.record_failure(host, &msg, true);
            }
            Ok((_, not_after)) => {
                self.backoff.lock().unwrap_or_else(PoisonError::into_inner).remove(host);
                info!("obtained a TLS certificate for {host} (valid until {not_after})");
            }
            Err(e) => {
                if e.account_gone {
                    self.forget_account().await;
                }
                self.record_failure(host, &e.message, true);
            }
        }
    }

    /// Returns `(renew_after, not_after)` of the new certificate.
    async fn obtain(
        &self,
        account: &Account,
        host: &str,
    ) -> std::result::Result<(DateTime<Utc>, DateTime<Utc>), IssueError> {
        let pem = acme::order_certificate(account, host, &self.challenges).await?;
        let issued_at = Utc::now();
        let mut issued = certs::issued_from_pem(
            pem.chain_pem.as_bytes(),
            pem.key_pem.as_bytes(),
            Some(issued_at),
            self.store.provider(),
        )
        .map_err(|e| IssueError::other(format!("the CA returned an unusable certificate: {e}")))?;
        let names = certs::dns_names(&issued.key.cert[0]).unwrap_or_default();
        if !certs::covers(&names, host) {
            return Err(IssueError::other(format!("the CA returned a certificate for {names:?}, not {host}")));
        }
        issued.directory_url = Some(self.directory_url.clone());
        let (renew_after, not_after) = (issued.renew_after(), issued.not_after);
        let meta = storage::CertMeta { issued_at, not_after, directory_url: Some(self.directory_url.clone()) };
        let (dir, h) = (self.config.certs_dir.clone(), host.to_string());
        let saved = tokio::task::spawn_blocking(move || {
            storage::save_certificate(&dir, &h, &pem.chain_pem, &pem.key_pem, &meta)
        })
        .await;
        match saved {
            Ok(Ok(())) => {}
            // Still serve it; it will be requested again after a restart.
            Ok(Err(e)) => warn!("cannot save the TLS certificate for {host}: {e}"),
            Err(e) => warn!("cannot save the TLS certificate for {host}: {e}"),
        }
        self.store.insert_issued(host, issued);
        Ok((renew_after, not_after))
    }

    /// Issued by another ACME directory than the configured one (e.g. a
    /// staging certificate after switching to production): replace it now.
    fn issued_elsewhere(&self, cert: &IssuedSummary) -> bool {
        cert.directory_url.as_deref().is_some_and(|url| url != self.directory_url)
    }

    /// The ACME account, restored or registered on first use and cached.
    async fn account(&self) -> std::result::Result<Account, IssueError> {
        let mut cached = self.account.lock().await;
        if let Some(account) = cached.as_ref() {
            return Ok(account.clone());
        }
        let load = acme::load_or_create_account(
            &self.acme_client,
            &self.config.certs_dir,
            &self.directory_url,
            &self.config.acme_email,
        );
        let account = tokio::time::timeout(ACCOUNT_TIMEOUT, load)
            .await
            .map_err(|_| IssueError::other(format!("timed out after {}s", ACCOUNT_TIMEOUT.as_secs())))??;
        *cached = Some(account.clone());
        Ok(account)
    }

    /// The CA no longer knows the cached account: drop it so the next
    /// attempt registers a new one.
    async fn forget_account(&self) {
        let mut cached = self.account.lock().await;
        if cached.take().is_some() {
            warn!("the ACME CA rejected our account; a new one will be registered");
            acme::retire_account_file(&self.config.certs_dir, &self.directory_url).await;
        }
    }

    fn record_failure(&self, host: &str, error: &str, log: bool) {
        let mut map = self.backoff.lock().unwrap_or_else(PoisonError::into_inner);
        let (entry, wait) = Backoff::fail(map.get(host), tokio::time::Instant::now(), error);
        if log {
            warn!(
                "could not obtain a TLS certificate for {host} (attempt {}): {error}; retrying in {}",
                entry.failures,
                backoff::human(wait)
            );
        }
        map.insert(host.to_string(), entry);
    }
}

impl TlsHooks for CertManager {
    fn http01_response(&self, token: &str) -> Option<String> {
        self.challenges.read().unwrap_or_else(PoisonError::into_inner).get(token).cloned()
    }

    fn has_certificate(&self, host: &str) -> bool {
        self.store.has_valid_issued(strip_port(host.trim()), Utc::now())
    }
}

fn build_server_config(store: &Arc<CertStore>) -> Arc<rustls::ServerConfig> {
    let resolver = Arc::new(SniResolver::new(store.clone()));
    let builder = match rustls::ServerConfig::builder_with_provider(store.provider().clone())
        .with_safe_default_protocol_versions()
    {
        Ok(builder) => builder,
        Err(e) => {
            warn!("the process crypto provider cannot serve TLS 1.2/1.3 ({e}); using ring");
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                // Invariant: ring's default provider supports every safe default version.
                .expect("ring supports TLS 1.2 and 1.3")
        }
    };
    let mut config = builder.with_no_client_auth().with_cert_resolver(resolver);
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Arc::new(config)
}

#[cfg(test)]
mod tests;
