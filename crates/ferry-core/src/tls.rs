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
