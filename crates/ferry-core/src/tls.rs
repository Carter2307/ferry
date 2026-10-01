//! Contract between the TLS/ACME manager (`ferry-tls`) and the proxy
//! (`ferry-proxy`), so neither crate depends on the other.

/// Implemented by the certificate manager, consumed by the proxy.
pub trait TlsHooks: Send + Sync + 'static {
    /// Key authorization to serve at `/.well-known/acme-challenge/{token}`
    /// (ACME HTTP-01), if a challenge with that token is pending.
    fn http01_response(&self, token: &str) -> Option<String>;

    /// True when a valid (CA-issued) certificate exists for `host`. The proxy
    /// redirects plain-HTTP requests for such hosts to HTTPS.
    fn has_certificate(&self, host: &str) -> bool;
}

/// What the certificate manager knows about the certificate of one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificateStatus {
    /// A valid certificate of the CA is served.
    Issued { not_after: chrono::DateTime<chrono::Utc> },
    /// An order is in flight.
    Issuing,
    /// No certificate yet: it is ordered at the next pass.
    Pending,
    /// The last order failed; the next one waits until `retry_at`.
    Failed { error: String, retry_at: Option<chrono::DateTime<chrono::Utc>> },
}

/// Implemented by the certificate manager, consumed by the API (which shows
/// the state of every routed host), so `ferry-api` doesn't depend on
/// `ferry-tls`.
pub trait Certificates: Send + Sync + 'static {
    /// The state of the certificate of `host` (a public name the proxy routes).
    fn certificate(&self, host: &str) -> CertificateStatus;
}
