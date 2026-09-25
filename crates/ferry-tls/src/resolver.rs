//! In-memory certificate store and the rustls SNI resolver built on it.
//!
//! Lookup order for a handshake:
//! 1. SNI (lowercased, trailing dot removed) → CA-issued certificate;
//! 2. SNI → cached self-signed certificate for that name (generated on first
//!    use, bounded cache);
//! 3. no SNI → the default self-signed certificate for [`DEFAULT_NAME`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use chrono::{DateTime, Duration, Utc};
use ferry_core::Result;
use rustls::crypto::CryptoProvider;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tracing::{debug, warn};

use crate::certs::{self, IssuedCert, SelfSigned};

/// Name of the fallback certificate served to clients that send no SNI.
pub(crate) const DEFAULT_NAME: &str = "ferry";
/// Maximum number of cached self-signed certificates.
pub(crate) const SELF_SIGNED_CACHE_CAPACITY: usize = 1000;
/// Regenerate a cached self-signed certificate this long before it expires.
const SELF_SIGNED_REFRESH_BEFORE_DAYS: i64 = 30;

/// Lowercase and strip a trailing dot.
pub(crate) fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Bounded FIFO cache of self-signed certificates.
struct SelfSignedCache {
    capacity: usize,
    entries: HashMap<String, SelfSigned>,
    order: VecDeque<String>,
}

impl SelfSignedCache {
    fn new(capacity: usize) -> Self {
        SelfSignedCache { capacity: capacity.max(1), entries: HashMap::new(), order: VecDeque::new() }
    }

    fn get_fresh(&self, name: &str, now: DateTime<Utc>) -> Option<Arc<CertifiedKey>> {
        self.entries
            .get(name)
            .filter(|e| e.not_after - now > Duration::days(SELF_SIGNED_REFRESH_BEFORE_DAYS))
            .map(|e| e.key.clone())
    }

    fn insert(&mut self, name: String, cert: SelfSigned) {
        if self.entries.insert(name.clone(), cert).is_some() {
            // Replaced (refreshed) in place: keep its position.
            return;
        }
        self.order.push_back(name);
        while self.entries.len() > self.capacity {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.entries.remove(&oldest);
                }
                None => break,
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// What the renewal logic needs to know about an issued certificate.
#[derive(Debug, Clone)]
pub(crate) struct IssuedSummary {
    pub renew_after: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    pub directory_url: Option<String>,
}

/// Every certificate the manager can serve.
pub(crate) struct CertStore {
    provider: Arc<CryptoProvider>,
    issued: RwLock<HashMap<String, IssuedCert>>,
    self_signed: Mutex<SelfSignedCache>,
}

impl CertStore {
    /// Create the store and generate the default certificate up front, so a
    /// broken crypto setup fails at startup instead of during handshakes.
    pub(crate) fn new(provider: Arc<CryptoProvider>, cache_capacity: usize) -> Result<Self> {
        let store = CertStore {
            provider,
            issued: RwLock::new(HashMap::new()),
            self_signed: Mutex::new(SelfSignedCache::new(cache_capacity)),
        };
        store.self_signed_for(DEFAULT_NAME)?;
        Ok(store)
    }

    pub(crate) fn provider(&self) -> &Arc<CryptoProvider> {
        &self.provider
    }

    /// Add or replace the CA-issued certificate of `host`.
    pub(crate) fn insert_issued(&self, host: &str, cert: IssuedCert) {
        self.issued.write().unwrap_or_else(PoisonError::into_inner).insert(normalize_host(host), cert);
    }

    /// `not_after` of the CA-issued certificate of `host`, if any.
    pub(crate) fn issued_not_after(&self, host: &str) -> Option<DateTime<Utc>> {
        self.issued_summary(host).map(|s| s.not_after)
    }

    /// Renewal facts about the CA-issued certificate of `host`.
    pub(crate) fn issued_summary(&self, host: &str) -> Option<IssuedSummary> {
        self.issued.read().unwrap_or_else(PoisonError::into_inner).get(&normalize_host(host)).map(|c| IssuedSummary {
            renew_after: c.renew_after(),
            not_after: c.not_after,
            directory_url: c.directory_url.clone(),
        })
    }

    /// True when `host` has a CA-issued certificate that has not expired.
    pub(crate) fn has_valid_issued(&self, host: &str, now: DateTime<Utc>) -> bool {
        self.issued_not_after(host).is_some_and(|na| na > now)
    }

    /// Number of CA-issued certificates.
    pub(crate) fn issued_count(&self) -> usize {
        self.issued.read().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// The certificate to present for a ClientHello with this SNI value.
    /// `None` only if a self-signed certificate cannot be generated.
    pub(crate) fn resolve_name(&self, sni: Option<&str>) -> Option<Arc<CertifiedKey>> {
        let name = sni.map(normalize_host).filter(|n| !n.is_empty());
        let Some(name) = name else {
            return self.self_signed_or_log(DEFAULT_NAME);
        };
        if let Some(cert) = self.issued.read().unwrap_or_else(PoisonError::into_inner).get(&name) {
            return Some(cert.key.clone());
        }
        self.self_signed_or_log(&name).or_else(|| self.self_signed_or_log(DEFAULT_NAME))
    }

    fn self_signed_or_log(&self, name: &str) -> Option<Arc<CertifiedKey>> {
        match self.self_signed_for(name) {
            Ok(key) => Some(key),
            Err(e) => {
                warn!("cannot generate a self-signed certificate for '{name}': {e}");
                None
            }
        }
    }

    /// Cached self-signed certificate for `name`, generated on first use.
    pub(crate) fn self_signed_for(&self, name: &str) -> Result<Arc<CertifiedKey>> {
        let now = Utc::now();
        if let Some(key) = self.self_signed.lock().unwrap_or_else(PoisonError::into_inner).get_fresh(name, now) {
            return Ok(key);
        }
        // Generate outside the lock: concurrent handshakes for other names are
        // not held up. Two racing handshakes for the same new name may both
        // generate; the last one wins the cache slot, which is harmless.
        debug!("generating self-signed certificate for '{name}'");
        let generated = certs::self_signed(name, &self.provider)?;
        let key = generated.key.clone();
        self.self_signed.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_string(), generated);
        Ok(key)
    }

    #[cfg(test)]
    pub(crate) fn self_signed_len(&self) -> usize {
        self.self_signed.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// rustls certificate resolver backed by a shared [`CertStore`]; certificates
/// added to the store later are picked up by existing server configs.
pub(crate) struct SniResolver {
    store: Arc<CertStore>,
}

impl SniResolver {
    pub(crate) fn new(store: Arc<CertStore>) -> Self {
        SniResolver { store }
    }
}

impl std::fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniResolver").field("issued", &self.store.issued_count()).finish_non_exhaustive()
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.store.resolve_name(client_hello.server_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::tests::provider;

    fn names_of(key: &CertifiedKey) -> Vec<String> {
        certs::dns_names(&key.cert[0]).unwrap()
    }

    fn issued(name: &str, days_left: i64) -> IssuedCert {
        let now = Utc::now();
        let (cert, key) =
            certs::self_signed_pem(name, now - Duration::days(1), now + Duration::days(days_left)).unwrap();
        certs::issued_from_pem(cert.as_bytes(), key.as_bytes(), None, &provider()).unwrap()
    }

    #[test]
    fn default_cert_without_sni() {
        let store = CertStore::new(provider(), 10).unwrap();
        let a = store.resolve_name(None).unwrap();
        assert_eq!(names_of(&a), vec![DEFAULT_NAME]);
        // cached: same key object every time
        assert!(Arc::ptr_eq(&a, &store.resolve_name(None).unwrap()));
        assert!(Arc::ptr_eq(&a, &store.resolve_name(Some("")).unwrap()));
    }

    #[test]
    fn self_signed_per_sni_lowercased_and_cached() {
        let store = CertStore::new(provider(), 10).unwrap();
        let a = store.resolve_name(Some("Web.Example.COM.")).unwrap();
        assert_eq!(names_of(&a), vec!["web.example.com"]);
        let b = store.resolve_name(Some("web.example.com")).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let c = store.resolve_name(Some("api.localhost")).unwrap();
        assert_eq!(names_of(&c), vec!["api.localhost"]);
        assert!(!store.has_valid_issued("web.example.com", Utc::now()));
    }

    #[test]
    fn issued_certificate_is_preferred() {
        let store = CertStore::new(provider(), 10).unwrap();
        let before = store.resolve_name(Some("shop.example.com")).unwrap();
        let cert = issued("shop.example.com", 60);
        let issued_key = cert.key.clone();
        store.insert_issued("SHOP.example.com", cert);
        let after = store.resolve_name(Some("shop.example.com")).unwrap();
        assert!(Arc::ptr_eq(&after, &issued_key));
        assert!(!Arc::ptr_eq(&after, &before));
        assert!(store.has_valid_issued("shop.example.com.", Utc::now()));
        assert!(store.issued_not_after("shop.example.com").is_some());
        // other names still get self-signed certs
        let other = store.resolve_name(Some("other.example.com")).unwrap();
        assert_eq!(names_of(&other), vec!["other.example.com"]);
    }

    #[test]
    fn expired_issued_is_not_valid_but_still_served() {
        let store = CertStore::new(provider(), 10).unwrap();
        let now = Utc::now();
        let (cert, key) =
            certs::self_signed_pem("old.example.com", now - Duration::days(100), now - Duration::days(2)).unwrap();
        let cert = certs::issued_from_pem(cert.as_bytes(), key.as_bytes(), None, &provider()).unwrap();
        let k = cert.key.clone();
        store.insert_issued("old.example.com", cert);
        assert!(!store.has_valid_issued("old.example.com", now));
        assert!(Arc::ptr_eq(&store.resolve_name(Some("old.example.com")).unwrap(), &k));
    }

    #[test]
    fn self_signed_cache_is_bounded() {
        let store = CertStore::new(provider(), 3).unwrap();
        for i in 0..10 {
            store.resolve_name(Some(&format!("h{i}.example.com"))).unwrap();
            assert!(store.self_signed_len() <= 3);
        }
        assert_eq!(store.self_signed_len(), 3);
        // the default certificate is regenerated on demand after eviction
        assert_eq!(names_of(&store.resolve_name(None).unwrap()), vec![DEFAULT_NAME]);
    }

    #[test]
    fn cache_refreshes_expiring_entries() {
        let mut cache = SelfSignedCache::new(2);
        let p = provider();
        let mut ss = certs::self_signed("a.example.com", &p).unwrap();
        let now = Utc::now();
        assert!(cache.get_fresh("a.example.com", now).is_none());
        cache.insert("a.example.com".into(), ss.clone());
        assert!(cache.get_fresh("a.example.com", now).is_some());
        ss.not_after = now + Duration::days(3);
        cache.insert("a.example.com".into(), ss);
        assert_eq!(cache.len(), 1);
        assert!(cache.get_fresh("a.example.com", now).is_none());
    }
}
