//! # ferry-proxy
//!
//! The public edge of Ferry: an HTTP/1.1 + HTTP/2 reverse proxy that routes
//! by `Host` header to service containers (published on `127.0.0.1:<port>`),
//! with round-robin load balancing, websocket/upgrade passthrough,
//! `X-Forwarded-*` headers, ACME HTTP-01 challenge answering, optional TLS
//! termination (SNI certificates provided by `ferry-tls`) and HTTP→HTTPS
//! redirects for hosts that have a certificate.
//!
//! The [`RouteTable`] is shared with the engine, which updates it atomically
//! after each deploy (blue/green swap).

use std::net::SocketAddr;
use std::sync::Arc;

use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Result};

/// Result of looking up a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Forward to this upstream (already round-robin picked).
    Upstream(SocketAddr),
    /// Service is suspended → 503 "Service suspended" page.
    Suspended,
    /// Known host but no healthy instance (e.g. first deploy in progress) → 503.
    NoUpstreams,
    /// Unknown host → 404 page.
    NotFound,
}

/// A route as seen by `snapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub host: String,
    pub service_id: String,
    pub upstreams: Vec<SocketAddr>,
    pub suspended: bool,
}

/// Shared, thread-safe host → upstreams table. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    inner: Arc<std::sync::RwLock<()>>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically make `hosts` (case-insensitive, port ignored) point at
    /// `upstreams` for `service_id`, removing any other host previously
    /// routed to that service. An empty `upstreams` yields `NoUpstreams`.
    pub fn set_service_routes(&self, service_id: &str, hosts: &[String], upstreams: Vec<SocketAddr>) {
        let _ = (service_id, hosts, upstreams, &self.inner);
        todo!("ferry-proxy: set_service_routes")
    }

    /// Route `hosts` of `service_id` to the "suspended" page.
    pub fn set_service_suspended(&self, service_id: &str, hosts: &[String]) {
        let _ = (service_id, hosts);
        todo!("ferry-proxy: set_service_suspended")
    }

    /// Remove every route of a service.
    pub fn remove_service(&self, service_id: &str) {
        let _ = service_id;
        todo!("ferry-proxy: remove_service")
    }

    /// Look up a `Host` header value (may include `:port`; case-insensitive).
    pub fn resolve(&self, host: &str) -> Resolution {
        let _ = host;
        todo!("ferry-proxy: resolve")
    }

    /// All routed hostnames (sorted) — used by the TLS manager.
    pub fn hosts(&self) -> Vec<String> {
        todo!("ferry-proxy: hosts")
    }

    /// Current routes (sorted by host), for debugging / status.
    pub fn snapshot(&self) -> Vec<RouteSnapshot> {
        todo!("ferry-proxy: snapshot")
    }
}

/// Proxy listeners and TLS settings.
#[derive(Clone)]
pub struct ProxyConfig {
    pub http_addr: SocketAddr,
    /// HTTPS listener (requires `tls`).
    pub https_addr: Option<SocketAddr>,
    /// rustls config with an SNI cert resolver (from `ferry-tls`).
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// ACME challenge answers + "has certificate" checks.
    pub tls_hooks: Option<Arc<dyn TlsHooks>>,
    /// Redirect HTTP → HTTPS for hosts where `tls_hooks.has_certificate(host)`.
    pub redirect_https: bool,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("http_addr", &self.http_addr)
            .field("https_addr", &self.https_addr)
            .field("tls", &self.tls.is_some())
            .field("redirect_https", &self.redirect_https)
            .finish()
    }
}

impl ProxyConfig {
    /// Plain HTTP only.
    pub fn http(addr: SocketAddr) -> Self {
        ProxyConfig { http_addr: addr, https_addr: None, tls: None, tls_hooks: None, redirect_https: false }
    }
}

/// Run the proxy until `shutdown` is cancelled (then drain gracefully, max ~10s).
/// Returns an error if a listener cannot bind.
pub async fn serve(config: ProxyConfig, routes: RouteTable, shutdown: CancellationToken) -> Result<()> {
    let _ = (config, routes, shutdown);
    todo!("ferry-proxy: serve")
}
