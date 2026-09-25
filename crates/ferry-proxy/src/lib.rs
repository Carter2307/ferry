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
//!
//! Request handling, in order:
//! 1. `GET /.well-known/acme-challenge/<token>` on plain HTTP (when
//!    `tls_hooks` is set) → the key authorization (200 `text/plain`) or 404.
//! 2. Host = HTTP/2 `:authority` / absolute-form URI authority, else `Host`;
//!    missing or invalid → 400.
//! 3. `redirect_https` + plain HTTP + `has_certificate(host)` → 308 to HTTPS.
//! 4. [`RouteTable::resolve`] → 404 / 503 (suspended) / 503 + `Retry-After`
//!    (no upstreams) error pages, or an upstream.
//! 5. Forward as HTTP/1.1 to `http://<upstream><path?query>` with the original
//!    `Host`, streamed bodies, hop-by-hop headers stripped and
//!    `X-Forwarded-{For,Proto,Host,Port}`, `X-Real-IP`, `X-Request-Id` set.
//!    Bodyless GET/HEAD requests are retried once on another upstream when
//!    the connection fails; otherwise failures → 502.
//! 6. `101 Switching Protocols` (websockets) → both sides are spliced.
//!
//! Proxy-generated error pages carry an `x-ferry-error: <kind>` header
//! (`bad_request`, `not_found`, `method_not_allowed`, `suspended`,
//! `no_upstreams`, `bad_gateway`).

use std::net::SocketAddr;
use std::sync::Arc;

use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Result};

mod body;
mod handler;
mod headers;
mod pages;
mod routes;
mod server;

#[cfg(test)]
mod tests;

pub use routes::{Resolution, RouteSnapshot, RouteTable, normalize_host};

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
///
/// Also returns an `Invalid` error when `https_addr` is set without `tls`.
/// `tls` without `https_addr` is ignored. Redirects to HTTPS only happen while
/// an HTTPS listener is running; they target its port.
pub async fn serve(config: ProxyConfig, routes: RouteTable, shutdown: CancellationToken) -> Result<()> {
    if let (Some(addr), None) = (config.https_addr, &config.tls) {
        return Err(ferry_core::Error::invalid(format!(
            "proxy HTTPS address {addr} is set but no TLS configuration was provided"
        )));
    }
    let http = server::bind(config.http_addr, "HTTP").await?;
    let https = match config.https_addr {
        Some(addr) => Some(server::bind(addr, "HTTPS").await?),
        None => None,
    };
    if config.tls.is_some() && https.is_none() {
        tracing::debug!("proxy: TLS configuration given without an HTTPS address; serving plain HTTP only");
    }
    server::serve_on(&config, http, https, routes, shutdown, server::DRAIN_TIMEOUT).await
}
