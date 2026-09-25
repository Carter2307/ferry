//! Certificate plumbing: PEM parsing, validity extraction, self-signed
//! certificates and rustls `CertifiedKey` construction.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, Utc};
use ferry_core::{Error, Result};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;

/// Validity of generated self-signed certificates (below the 398-day cap some
/// clients enforce even for user-trusted certificates).
pub(crate) const SELF_SIGNED_VALIDITY_DAYS: i64 = 397;

/// A certificate issued by the ACME CA, ready to be served.
#[derive(Clone)]
pub(crate) struct IssuedCert {
    pub key: Arc<CertifiedKey>,
    pub issued_at: DateTime<Utc>,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    /// ACME directory that issued it (`None`: unknown, e.g. placed by hand).
    pub directory_url: Option<String>,
}

impl IssuedCert {
    /// When this certificate becomes due for renewal (see [`renew_after`]).
    pub(crate) fn renew_after(&self) -> DateTime<Utc> {
        renew_after(self.not_before, self.not_after)
    }
}

/// Renewal time of a certificate: [`crate::RENEW_BEFORE_DAYS`] before it
/// expires, or — for short-lived certificates (lifetime under 90 days) — when
/// the last third of its lifetime starts, so that e.g. 6-day certificates
/// aren't re-ordered on every pass.
pub(crate) fn renew_after(not_before: DateTime<Utc>, not_after: DateTime<Utc>) -> DateTime<Utc> {
    let lifetime = (not_after - not_before).max(Duration::zero());
    not_after - Duration::days(crate::RENEW_BEFORE_DAYS).min(lifetime / 3)
}

/// True when `names` (DNS SANs) cover `host`, exactly or via a `*.` wildcard
/// for a single label.
pub(crate) fn covers(names: &[String], host: &str) -> bool {
    names.iter().any(|n| {
        n == host
            || n.strip_prefix("*.").is_some_and(|parent| {
                host.split_once('.').is_some_and(|(label, rest)| !label.is_empty() && rest == parent)
            })
    })
}

impl std::fmt::Debug for IssuedCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedCert")
            .field("issued_at", &self.issued_at)
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .field("directory_url", &self.directory_url)
            .field("chain_len", &self.key.cert.len())
            .finish()
    }
}

/// A generated self-signed certificate.
#[derive(Clone)]
pub(crate) struct SelfSigned {
    pub key: Arc<CertifiedKey>,
    pub not_after: DateTime<Utc>,
}

/// All certificates of a PEM bundle, leaf first. Errors when there are none.
pub(crate) fn parse_chain_pem(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = pem;
    let chain = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| Error::invalid(format!("parsing certificate PEM: {e}")))?;
    if chain.is_empty() {
        return Err(Error::invalid("certificate PEM contains no certificate"));
    }
    Ok(chain)
}

/// The first private key (PKCS#8, SEC1 or PKCS#1) of a PEM file.
pub(crate) fn parse_key_pem(pem: &[u8]) -> Result<PrivateKeyDer<'static>> {
    let mut reader = pem;
    rustls_pemfile::private_key(&mut reader)
        .map_err(|e| Error::invalid(format!("parsing private key PEM: {e}")))?
        .ok_or_else(|| Error::invalid("key PEM contains no private key"))
}

/// Parse a DER certificate.
fn parse_x509<'a>(der: &'a CertificateDer<'_>) -> Result<x509_parser::certificate::X509Certificate<'a>> {
    x509_parser::parse_x509_certificate(der.as_ref())
        .map(|(_, cert)| cert)
        .map_err(|e| Error::invalid(format!("parsing X.509 certificate: {e}")))
}

fn timestamp(secs: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(secs, 0).ok_or_else(|| Error::invalid(format!("certificate date {secs} out of range")))
}

/// `(not_before, not_after)` of a DER certificate.
pub(crate) fn validity(der: &CertificateDer<'_>) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let cert = parse_x509(der)?;
    let v = cert.validity();
    Ok((timestamp(v.not_before.timestamp())?, timestamp(v.not_after.timestamp())?))
}

/// DNS names in the subject alternative name extension of a DER certificate.
pub(crate) fn dns_names(der: &CertificateDer<'_>) -> Result<Vec<String>> {
    let cert = parse_x509(der)?;
    let san = cert.subject_alternative_name().map_err(|e| Error::invalid(format!("parsing SAN extension: {e}")))?;
    Ok(san
        .map(|ext| {
            ext.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(d) => Some(d.to_ascii_lowercase()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Build a rustls `CertifiedKey`, checking that the key matches the leaf.
pub(crate) fn certified_key(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    provider: &CryptoProvider,
) -> Result<CertifiedKey> {
    CertifiedKey::from_der(chain, key, provider).map_err(|e| Error::invalid(format!("loading certificate key: {e}")))
}

/// Load an issued certificate from its PEM chain and key. `issued_at`
/// defaults to the leaf's `notBefore`.
pub(crate) fn issued_from_pem(
    chain_pem: &[u8],
    key_pem: &[u8],
    issued_at: Option<DateTime<Utc>>,
    provider: &CryptoProvider,
) -> Result<IssuedCert> {
    let chain = parse_chain_pem(chain_pem)?;
    let (not_before, not_after) = validity(&chain[0])?;
    let key = certified_key(chain, parse_key_pem(key_pem)?, provider)?;
    Ok(IssuedCert {
        key: Arc::new(key),
        issued_at: issued_at.unwrap_or(not_before),
        not_before,
        not_after,
        directory_url: None,
    })
}

/// A fresh ECDSA P-256 key pair.
pub(crate) fn generate_key() -> Result<KeyPair> {
    KeyPair::generate().map_err(|e| Error::internal(format!("generating key pair: {e}")))
}

/// DER-encoded CSR for `host` signed by `key` (empty subject, host as SAN).
pub(crate) fn csr_der(host: &str, key: &KeyPair) -> Result<Vec<u8>> {
    let mut params = CertificateParams::new(vec![host.to_string()])
        .map_err(|e| Error::invalid(format!("CSR parameters for {host}: {e}")))?;
    params.distinguished_name = DistinguishedName::new();
    let csr = params.serialize_request(key).map_err(|e| Error::internal(format!("creating CSR for {host}: {e}")))?;
    Ok(csr.der().to_vec())
}

/// Generate a self-signed certificate for `name`, valid from yesterday for
/// [`SELF_SIGNED_VALIDITY_DAYS`].
pub(crate) fn self_signed(name: &str, provider: &CryptoProvider) -> Result<SelfSigned> {
    let now = Utc::now();
    let (cert, key) =
        build_self_signed(name, now - Duration::days(1), now + Duration::days(SELF_SIGNED_VALIDITY_DAYS))?;
    let leaf = cert.der().clone();
    let (_, not_after) = validity(&leaf)?;
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let key = certified_key(vec![leaf], key_der, provider)?;
    Ok(SelfSigned { key: Arc::new(key), not_after })
}

/// PEM `(certificate, private key)` of a self-signed certificate for `name`
/// with the given validity window (truncated to whole days).
#[cfg(test)]
pub(crate) fn self_signed_pem(
    name: &str,
    not_before: DateTime<Utc>,
    not_after: DateTime<Utc>,
) -> Result<(String, String)> {
    let (cert, key) = build_self_signed(name, not_before, not_after)?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn build_self_signed(
    name: &str,
    not_before: DateTime<Utc>,
    not_after: DateTime<Utc>,
) -> Result<(rcgen::Certificate, KeyPair)> {
    let key = generate_key()?;
    let mut params = CertificateParams::new(vec![name.to_string()])
        .map_err(|e| Error::invalid(format!("self-signed certificate for '{name}': {e}")))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    dn.push(DnType::OrganizationName, "Ferry (self-signed)");
    params.distinguished_name = dn;
    // rcgen takes calendar dates: truncate to the UTC day. chrono months
    // (1-12) and days (1-31) always fit in u8, and a date chrono produced is
    // always a valid calendar date for `date_time_ymd` (which panics otherwise).
    let day = |t: DateTime<Utc>| rcgen::date_time_ymd(t.year(), t.month() as u8, t.day() as u8);
    params.not_before = day(not_before);
    params.not_after = day(not_after);
    let cert = params
        .self_signed(&key)
        .map_err(|e| Error::internal(format!("signing self-signed certificate for '{name}': {e}")))?;
    Ok((cert, key))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn provider() -> Arc<CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    #[test]
    fn self_signed_covers_name_and_is_currently_valid() {
        let p = provider();
        let ss = self_signed("app.example.com", &p).unwrap();
        let leaf = &ss.key.cert[0];
        assert_eq!(dns_names(leaf).unwrap(), vec!["app.example.com"]);
        let (nb, na) = validity(leaf).unwrap();
        let now = Utc::now();
        assert!(nb <= now && now < na);
        assert!(na > now + Duration::days(300));
        assert_eq!(ss.not_after, na);
    }

    #[test]
    fn pem_round_trip_and_validity() {
        let p = provider();
        let nb = Utc::now() - Duration::days(10);
        let na = Utc::now() + Duration::days(80);
        let (cert, key) = self_signed_pem("x.example.org", nb, na).unwrap();
        let issued = issued_from_pem(cert.as_bytes(), key.as_bytes(), None, &p).unwrap();
        assert_eq!(issued.not_after.date_naive(), na.date_naive());
        assert_eq!(issued.issued_at.date_naive(), nb.date_naive());
        assert_eq!(issued.key.cert.len(), 1);
    }

    #[test]
    fn rejects_garbage_and_mismatched_keys() {
        let p = provider();
        assert!(parse_chain_pem(b"").is_err());
        assert!(parse_chain_pem(b"-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n").is_err());
        assert!(parse_key_pem(b"no key here").is_err());
        let now = Utc::now();
        let (cert_a, _) = self_signed_pem("a.example.com", now, now + Duration::days(5)).unwrap();
        let (_, key_b) = self_signed_pem("b.example.com", now, now + Duration::days(5)).unwrap();
        assert!(issued_from_pem(cert_a.as_bytes(), key_b.as_bytes(), None, &p).is_err());
    }

    #[test]
    fn renewal_time_depends_on_lifetime() {
        let t0 = Utc::now();
        // 90-day certificates: 30 days before expiry.
        assert_eq!(renew_after(t0, t0 + Duration::days(90)), t0 + Duration::days(60));
        // Longer ones too.
        assert_eq!(renew_after(t0, t0 + Duration::days(365)), t0 + Duration::days(335));
        // Short-lived: the last third.
        assert_eq!(renew_after(t0, t0 + Duration::days(45)), t0 + Duration::days(30));
        assert_eq!(renew_after(t0, t0 + Duration::days(6)), t0 + Duration::days(4));
        // Nonsense validity: due at expiry at the latest.
        assert_eq!(renew_after(t0, t0 - Duration::days(1)), t0 - Duration::days(1));
    }

    #[test]
    fn san_coverage() {
        let names = vec!["app.example.com".to_string(), "*.apps.example.com".to_string()];
        assert!(covers(&names, "app.example.com"));
        assert!(covers(&names, "web.apps.example.com"));
        assert!(!covers(&names, "apps.example.com"));
        assert!(!covers(&names, "a.b.apps.example.com"));
        assert!(!covers(&names, "other.example.com"));
        assert!(!covers(&[], "app.example.com"));
    }

    #[test]
    fn csr_is_generated() {
        let key = generate_key().unwrap();
        let der = csr_der("shop.example.com", &key).unwrap();
        assert!(!der.is_empty());
        // P-256 keys, as required.
        assert_eq!(key.algorithm(), &rcgen::PKCS_ECDSA_P256_SHA256);
    }
}
