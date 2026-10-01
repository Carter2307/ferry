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
//!    `GET /.well-known/ferry-domain-check/<token>` (when `domain_probe_id`
//!    is set, on either listener, whatever the host) → `<token>.<probe id>`
//!    (200 `text/plain`): how a server verifies that a name reaches it
//!    (`ferry_core::domains`).
//! 2. Host = HTTP/2 `:authority` / absolute-form URI authority, else `Host`;
//!    missing or invalid → 400.
//! 3. `redirect_https` + plain HTTP + `has_certificate(host)` → 308 to HTTPS.
//! 4. [`RouteTable::resolve`] → 404 / 503 (suspended) / 503 + `Retry-After`
//!    (no upstreams) error pages, or an upstream.
//! 5. Forward as HTTP/1.1 to `http://<upstream><path?query>` with the original
//!    `Host`, streamed bodies (re-framed: chunked when the length is unknown),
//!    hop-by-hop headers stripped, client-supplied forwarding headers
//!    (`Forwarded`, every `X-Forwarded-*`, `X-Real-IP`…) dropped, and
//!    `Forwarded`, `X-Forwarded-{For,Proto,Host,Port}`, `X-Real-IP` set from
//!    what the proxy saw (it is the edge: `X-Forwarded-For` is the client's
//!    address alone); `X-Request-Id` is set unless present.
//!    `X-Forwarded-Port` is, by design, the **client-facing** port: the port
//!    of the `Host` / `:authority` the client used, else the scheme's default
//!    (80 / 443) — consistent with `X-Forwarded-Host` — not the port of the
//!    listener that accepted the connection. Behind port forwarding (a router
//!    mapping 443 → 8443, a container publishing 80 → 8080) the listener's
//!    port is not the one clients connect to, and apps building absolute URLs
//!    from it would produce broken links.
//!    Bodyless GET/HEAD requests are retried once on another upstream when
//!    the connection fails; otherwise failures → 502. A client that stops
//!    sending its request body for [`ConnectionLimits::request_body_timeout`]
//!    gets a 408 (or, when the upstream has already started answering, the
//!    exchange is aborted).
//! 6. `101 Switching Protocols` (websockets, HTTP/1.1 only) → both sides are
//!    spliced.
//!
//! Proxy-generated error pages carry an `x-ferry-error: <kind>` header
//! (`bad_request`, `not_found`, `method_not_allowed`, `request_timeout`,
//! `suspended`, `no_upstreams`, `bad_gateway`). The `no_upstreams` page does
//! not guess why a service has no instance to route to (never deployed,
//! last deploy failed, deploy in progress): the route table doesn't know.
//!
//! Client connections are bounded by [`ConnectionLimits`]: a global cap
//! (from the open-file limit by default) and a per-address cap, a TLS
//! handshake timeout, a deadline for the first request head (whatever the
//! protocol turns out to be), HTTP/1 header read timeouts, an idle timeout
//! for connections without a request in flight (HTTP/2 included) that
//! shrinks while the cap is reached, HTTP/2 keep-alive pings, and an idle
//! timeout between the pieces of a request body (a body trickled then
//! stalled can't hold a connection forever). Otherwise requests in flight
//! (SSE and other streamed responses, uploads that keep sending) and
//! websocket tunnels are never timed out; TCP keep-alive probes close them
//! once the client machine is gone.

use std::net::SocketAddr;
use std::sync::Arc;

use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Result};

mod body;
mod handler;
mod headers;
mod limits;
mod pages;
mod routes;
mod server;

#[cfg(test)]
mod tests;

pub use limits::ConnectionLimits;
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
    /// What this server answers the verification of a domain with
    /// (`Config::domains.probe_id()`); `None` = the path is routed like any
    /// other.
    pub domain_probe_id: Option<String>,
    /// Connection caps and timeouts (both listeners).
    pub limits: ConnectionLimits,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("http_addr", &self.http_addr)
            .field("https_addr", &self.https_addr)
            .field("tls", &self.tls.is_some())
            .field("redirect_https", &self.redirect_https)
            .field("limits", &self.limits)
            .finish()
    }
}

impl ProxyConfig {
    /// Plain HTTP only.
    pub fn http(addr: SocketAddr) -> Self {
        ProxyConfig {
            http_addr: addr,
            https_addr: None,
            tls: None,
            tls_hooks: None,
            redirect_https: false,
            domain_probe_id: None,
            limits: ConnectionLimits::default(),
        }
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
