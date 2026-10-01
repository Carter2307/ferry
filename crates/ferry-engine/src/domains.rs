//! Domains (DESIGN.md §21): keeps the hostnames of every service in step
//! with the domains of the store, and verifies that the names under a
//! domain reach this server.
//!
//! A verification asks two things about a made-up name under the domain
//! (`ferry-check-<random>.<domain>`, which only a wildcard record answers
//! for, and which no resolver has in its cache):
//!
//! 1. **DNS** — what does it resolve to? Compared with the server's public
//!    addresses when they are known.
//! 2. **HTTP** — does *this* server answer a request for it on the public
//!    HTTP port? The proxy answers `<token>.<probe id>`
//!    (`ferry_core::domains`), so another server, a parked page or a closed
//!    port are told apart.
//!
//! The names reach the server when it answered — or, when it couldn't reach
//! itself (a network that doesn't route a machine back to its own public
//! address), when the DNS points at one of its public addresses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferry_core::domains::{PROBE_PATH, new_probe_token, probe_answer, probe_host};
use ferry_core::{CheckOutcome, Domain, DomainCheck, DomainCheckKind, DomainStatus, Error, Result};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tracing::{debug, info, warn};

use crate::reconcile;
use crate::state::Inner;
use crate::util::lock;

/// How often the verifier looks for domains that are due.
const TICK: Duration = Duration::from_secs(5);
/// A domain that waits for its DNS is verified this often during its first
/// hour, then every minute for a day, then like an active one.
const PENDING_INTERVAL: Duration = Duration::from_secs(15);
const SLOW_INTERVAL: Duration = Duration::from_secs(60);
/// A working domain is verified again this often.
const ACTIVE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Verifications that must fail in a row before a working domain is flagged:
/// one lost DNS answer is not a misconfiguration.
const FAILURES_BEFORE_MISCONFIGURED: u32 = 3;
/// Bounds of one verification request.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// Addresses of a name that are asked, at most.
const MAX_ADDRESSES: usize = 4;
/// How long the server's public addresses are remembered once found, and
/// how soon they are looked for again when they were not.
const PUBLIC_TTL: Duration = Duration::from_secs(30 * 60);
const PUBLIC_RETRY: Duration = Duration::from_secs(2 * 60);
/// Services that answer a request with the address it came from (plain
/// text), asked in this order when the machine's own address is a private
/// one. `--public-ip` skips them.
const ADDRESS_ECHOES: [&str; 3] =
    ["https://api.ipify.org", "https://ipv4.icanhazip.com", "https://checkip.amazonaws.com"];

/// Why a verification request got no answer from this server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeError {
    TimedOut,
    /// Nothing accepted the connection (refused, unreachable…).
    Connect(String),
    /// Something answered, and not this server: its HTTP status.
    OtherServer(u16),
    Other(String),
}

/// What a verification needs from the network (replaced in tests).
#[async_trait]
pub(crate) trait Network: Send + Sync + 'static {
    /// The addresses `host` resolves to (never empty).
    async fn resolve(&self, host: &str) -> std::result::Result<Vec<IpAddr>, String>;
    /// `GET http://<host>:<port><path>` sent to `addr`: the body of a 200.
    async fn get(&self, host: &str, addr: SocketAddr, path: &str) -> std::result::Result<String, ProbeError>;
    /// The addresses this machine is reached at from the internet.
    async fn public_addresses(&self) -> Vec<IpAddr>;
}

/// The engine's domain state.
pub(crate) struct State {
    net: StdMutex<Arc<dyn Network>>,
    /// Wakes the verifier: a domain was connected or changed.
    pub wake: Notify,
    /// One verification at a time.
    verifying: AsyncMutex<()>,
    /// The public addresses found, and when.
    public: AsyncMutex<Option<(Instant, Vec<IpAddr>)>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            net: StdMutex::new(Arc::new(Internet)),
            wake: Notify::new(),
            verifying: AsyncMutex::new(()),
            public: AsyncMutex::new(None),
        }
    }
}

impl State {
    fn net(&self) -> Arc<dyn Network> {
        lock(&self.net).clone()
    }

    #[cfg(test)]
    pub(crate) fn set_network(&self, net: Arc<dyn Network>) {
        *lock(&self.net) = net;
    }
}

// ---------------------------------------------------------------------------
// one verification

/// What a verification found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// The names under the domain reach this server.
    pub ok: bool,
    pub checks: Vec<DomainCheck>,
}

fn list(addresses: &[IpAddr]) -> String {
    let all: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
    match all.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

fn on_port(port: u16) -> String {
    format!("port {port}")
}

/// Verify `domain` once: resolve a made-up name under it, then ask whoever
/// answers at its addresses whether it is this server. `expected` are the
/// server's public addresses (empty when unknown).
pub(crate) async fn check(net: &dyn Network, domain: &str, port: u16, probe_id: &str, expected: &[IpAddr]) -> Outcome {
    use CheckOutcome::{Failed, Passed, Skipped, Warning};
    use DomainCheckKind::{Dns, Http};
    let host = probe_host(domain);
    let wildcard = format!("*.{domain}");
    let here = list(expected);

    let addresses = match net.resolve(&host).await {
        Ok(addresses) if !addresses.is_empty() => addresses,
        outcome => {
            if let Err(e) = outcome {
                debug!(%domain, "resolving {host}: {e}");
            }
            let dns = format!("No DNS record answers for {wildcard} yet. A new record can take a few minutes to show.");
            let http = "Not checked: the name does not resolve.";
            return Outcome {
                ok: false,
                checks: vec![DomainCheck::new(Dns, Failed, dns), DomainCheck::new(Http, Skipped, http)],
            };
        }
    };
    let found = list(&addresses);
    // Only an address of a family this server knows its own address of can
    // be compared (its IPv6 address is only known when `--public-ip` says it).
    let (judged, unjudged): (Vec<IpAddr>, Vec<IpAddr>) =
        addresses.iter().copied().partition(|a| expected.iter().any(|e| e.is_ipv4() == a.is_ipv4()));
    let ours: Vec<IpAddr> = judged.iter().copied().filter(|a| expected.contains(a)).collect();
    // Every address that can be compared must be one of ours: a second
    // record that points elsewhere would send part of the visitors elsewhere.
    let points_here = !judged.is_empty() && ours.len() == judged.len();

    // IPv4 first: a server without IPv6 connectivity can't ask its own AAAA.
    let mut order: Vec<IpAddr> = addresses.iter().copied().filter(IpAddr::is_ipv4).collect();
    order.extend(addresses.iter().copied().filter(IpAddr::is_ipv6));
    let token = new_probe_token();
    let (want, path) = (probe_answer(&token, probe_id), format!("{PROBE_PATH}{token}"));
    let mut reached = false;
    let mut failure: Option<(IpAddr, ProbeError)> = None;
    for addr in order.into_iter().take(MAX_ADDRESSES) {
        match net.get(&host, SocketAddr::new(addr, port), &path).await {
            Ok(body) if body.trim() == want => {
                reached = true;
                break;
            }
            // A 200 with another body: a page of someone else, or another
            // Ferry server (its own probe id).
            Ok(_) => failure = failure.or(Some((addr, ProbeError::OtherServer(200)))),
            Err(e) => failure = failure.or(Some((addr, e))),
        }
    }

    let private = addresses.iter().all(|a| !is_public(*a));
    let dns = if points_here {
        let also = match unjudged.as_slice() {
            [] => String::new(),
            other => format!(
                " It also resolves to {}, which was not compared: set --public-ip to say this server's IPv6 address.",
                list(other)
            ),
        };
        DomainCheck::new(Dns, Passed, format!("{wildcard} resolves to {}, this server.{also}", list(&ours)))
    } else if reached && private {
        DomainCheck::new(
            Dns,
            Warning,
            format!(
                "{wildcard} resolves to {found}, a private address: its names reach this server from here, not from the internet."
            ),
        )
    } else if judged.is_empty() && reached {
        DomainCheck::new(Dns, Passed, format!("{wildcard} resolves to {found}."))
    } else if judged.is_empty() {
        DomainCheck::new(Dns, Warning, format!("{wildcard} resolves to {found}. Is that the address of this server?"))
    } else if !ours.is_empty() {
        // Some records are ours and some are not: visitors are spread over all of them.
        let message = format!(
            "{wildcard} resolves to {}, and only {} is this server: visitors sent to the others don't reach it. Remove the other records.",
            list(&judged),
            list(&ours)
        );
        DomainCheck::new(Dns, if reached { Warning } else { Failed }, message)
    } else if reached {
        DomainCheck::new(
            Dns,
            Warning,
            format!(
                "{wildcard} resolves to {found}, not to this server's address ({here}): something in between forwards to it, like a CDN or a load balancer."
            ),
        )
    } else {
        DomainCheck::new(
            Dns,
            Failed,
            format!("{wildcard} resolves to {found}, not to this server ({here}). Point the record at {here}."),
        )
    };

    let http = if reached {
        DomainCheck::new(Http, Passed, format!("This server answers on {} for names under {domain}.", on_port(port)))
    } else {
        let (addr, error) = failure.unwrap_or((addresses[0], ProbeError::Other("no answer".into())));
        let p = on_port(port);
        let (outcome, message) = match error {
            // Our own address, and nothing came back: nothing says visitors
            // are refused too.
            ProbeError::TimedOut | ProbeError::Connect(_) if points_here => (
                Warning,
                format!(
                    "This server could not reach itself at {addr} on {p}. Some networks don't allow that; if visitors can't reach it either, open {p} in the firewall."
                ),
            ),
            ProbeError::TimedOut => (Failed, format!("No answer from {addr} on {p} (timed out).")),
            ProbeError::Connect(e) => (Failed, format!("Could not connect to {addr} on {p}: {e}.")),
            ProbeError::OtherServer(status) => (
                Failed,
                format!(
                    "Another server answers at {addr} on {p} (HTTP {status}): requests for {wildcard} don't reach this one."
                ),
            ),
            ProbeError::Other(e) => (Failed, format!("The request to {addr} on {p} failed: {e}.")),
        };
        DomainCheck::new(Http, outcome, message)
    };
    // The DNS pointing at this server is the domain's whole part of the
    // setup: a closed port is the server's, and shows as a warning.
    let ok = reached || (points_here && http.outcome == Warning);
    Outcome { ok, checks: vec![dns, http] }
}

/// Record `outcome` on the domain: its checks, and its status — `active` as
/// soon as its names reach the server, `misconfigured` once that failed
/// [`FAILURES_BEFORE_MISCONFIGURED`] times in a row for an active domain.
/// A pending domain stays pending until it works.
pub(crate) fn apply(domain: &mut Domain, outcome: Outcome, now: DateTime<Utc>) {
    domain.checked_at = Some(now);
    domain.checks = outcome.checks;
    if outcome.ok {
        domain.failures = 0;
        domain.verified_at = Some(now);
        domain.status = DomainStatus::Active;
    } else {
        domain.failures = domain.failures.saturating_add(1);
        if domain.status == DomainStatus::Active && domain.failures >= FAILURES_BEFORE_MISCONFIGURED {
            domain.status = DomainStatus::Misconfigured;
        }
    }
}

/// How long after a verification the next one is due.
fn interval(domain: &Domain, now: DateTime<Utc>) -> Duration {
    match domain.status {
        DomainStatus::Pending => match (now - domain.created_at).num_minutes() {
            ..60 => PENDING_INTERVAL,
            60..1440 => SLOW_INTERVAL,
            _ => ACTIVE_INTERVAL,
        },
        DomainStatus::Misconfigured => SLOW_INTERVAL,
        DomainStatus::Active => ACTIVE_INTERVAL,
    }
}

/// Whether the domain should be verified now. Local domains never are.
pub(crate) fn due(domain: &Domain, now: DateTime<Utc>) -> bool {
    if domain.is_local() {
        return false;
    }
    let Some(checked_at) = domain.checked_at else { return true };
    (now - checked_at).to_std().is_ok_and(|elapsed| elapsed >= interval(domain, now))
}

// ---------------------------------------------------------------------------
// the engine's side

/// The port visitors use for plain HTTP.
fn public_http_port(inner: &Inner) -> u16 {
    inner.config.public_port.unwrap_or(inner.config.proxy_addr.port())
}

/// `Engine::public_addresses`: the configured ones, else what was found
/// (asked again after [`PUBLIC_TTL`], sooner when nothing was found).
pub(crate) async fn public_addresses(inner: &Inner) -> Vec<IpAddr> {
    if !inner.config.public_ips.is_empty() {
        return inner.config.public_ips.clone();
    }
    let mut cache = inner.domains.public.lock().await;
    if let Some((at, addresses)) = cache.as_ref() {
        let fresh = if addresses.is_empty() { PUBLIC_RETRY } else { PUBLIC_TTL };
        if at.elapsed() < fresh {
            return addresses.clone();
        }
    }
    let addresses = inner.domains.net().public_addresses().await;
    match addresses.as_slice() {
        [] => warn!(
            "could not find out the public address of this server: the DNS records of a domain are shown without it (set --public-ip)"
        ),
        found => info!(addresses = %list(found), "public address of this server"),
    }
    *cache = Some((Instant::now(), addresses.clone()));
    addresses
}

/// `Engine::refresh_domains`: read the domains of the store again and serve
/// every service under the ones that are served now. Only the hostnames of
/// the routes change here (their upstreams stay); the reconciler, woken,
/// then looks at each service as usual.
pub(crate) async fn refresh(inner: &Arc<Inner>) -> Result<()> {
    let before = inner.config.served_domains();
    ferry_core::domains::reload(&inner.store, &inner.config).await?;
    let after = inner.config.served_domains();
    for svc in inner.store.list_services().await? {
        if inner.is_service_deleting(&svc.id) {
            continue;
        }
        // Not while something else changes the service's routes, unless it
        // is a deploy: that one installs them with the current hosts itself.
        let _lock = inner.service_locks.try_lock(&svc.id);
        reconcile::rehost_routes(inner, &svc);
    }
    if before != after {
        info!(domains = %after.join(", "), "services are served under these domains");
    }
    inner.wake_reconciler();
    inner.domains.wake.notify_one();
    Ok(())
}

/// A domain started to work: it becomes the default one when the default
/// was only a local name (or a domain that is not served) — the URL of a
/// service should be one its visitors can open.
async fn promote(inner: &Inner, domain: &Domain) -> Result<()> {
    let domains = inner.store.list_domains().await?;
    let usable = domains.iter().any(|d| d.is_default && d.is_served() && !d.is_local());
    if !usable && !domain.is_default {
        inner.store.set_default_domain(&domain.id).await?;
        info!(domain = %domain.name, "the default domain is now this one: its DNS points at this server");
    }
    Ok(())
}

/// `Engine::verify_domain`: verify now, store what was found, and serve the
/// services under the domain as soon as it works.
pub(crate) async fn verify(inner: &Arc<Inner>, id_or_name: &str) -> Result<Domain> {
    let _one = inner.domains.verifying.lock().await;
    let mut domain = inner.store.require_domain(id_or_name).await?;
    if domain.is_local() {
        return Ok(domain);
    }
    let expected = public_addresses(inner).await;
    let port = public_http_port(inner);
    let net = inner.domains.net();
    let outcome = check(net.as_ref(), &domain.name, port, inner.config.domains.probe_id(), &expected).await;
    let before = domain.status;
    apply(&mut domain, outcome, ferry_core::now());
    let domain = inner.store.record_domain_check(&domain).await?;
    if domain.status != before {
        match domain.status {
            DomainStatus::Active => info!(domain = %domain.name, "the domain reaches this server"),
            DomainStatus::Misconfigured => {
                let why = domain.checks.iter().find(|c| c.outcome == CheckOutcome::Failed).map(|c| c.message.as_str());
                warn!(domain = %domain.name, "the domain no longer reaches this server: {}", why.unwrap_or("unknown"));
            }
            DomainStatus::Pending => {}
        }
        if domain.status == DomainStatus::Active && before == DomainStatus::Pending {
            promote(inner, &domain).await?;
        }
        refresh(inner).await?;
        return inner.store.require_domain(&domain.id).await;
    }
    Ok(domain)
}

/// The verifier: every few seconds (or when woken), verifies the domains
/// that are due.
pub(crate) async fn run_verifier(inner: Arc<Inner>) {
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(TICK) => {}
            _ = inner.domains.wake.notified() => {}
        }
        let domains = match inner.store.list_domains().await {
            Ok(domains) => domains,
            Err(e) => {
                debug!("domain verifier: reading the domains failed: {e}");
                continue;
            }
        };
        let now = Utc::now();
        for domain in domains.iter().filter(|d| due(d, now)) {
            let verified = tokio::select! {
                _ = inner.shutdown.cancelled() => return,
                r = verify(&inner, &domain.id) => r,
            };
            match verified {
                // Removed meanwhile.
                Ok(_) | Err(Error::NotFound(_)) => {}
                Err(e) => warn!(domain = %domain.name, "verifying the domain failed: {e}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the real network

struct Internet;

/// An address that is routed on the internet.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                // Carrier-grade NAT (100.64.0.0/10), benchmarks (198.18.0.0/15), reserved (240.0.0.0/4).
                || (a == 100 && (b & 0xc0) == 64)
                || (a == 198 && (b & 0xfe) == 18)
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            // Global unicast (2000::/3), minus the documentation prefix.
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

/// The address of the interface that routes to `target` (no packet is sent:
/// connecting a UDP socket only picks the route).
async fn outbound_address(target: SocketAddr) -> Option<IpAddr> {
    let any: SocketAddr = match target {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = tokio::net::UdpSocket::bind(any).await.ok()?;
    socket.connect(target).await.ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// Ask the address echo services, over IPv4, for the address they see.
async fn echoed_address() -> Option<IpAddr> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;
    for url in ADDRESS_ECHOES {
        let text = match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
            Ok(resp) => {
                debug!("public address: {url} answered {}", resp.status());
                continue;
            }
            Err(e) => {
                debug!("public address: {url}: {e}");
                continue;
            }
        };
        match text.trim().parse::<IpAddr>() {
            Ok(ip) if is_public(ip) => return Some(ip),
            _ => debug!("public address: {url} did not answer with a public address"),
        }
    }
    None
}

#[async_trait]
impl Network for Internet {
    async fn resolve(&self, host: &str) -> std::result::Result<Vec<IpAddr>, String> {
        let lookup = tokio::net::lookup_host((host, 80));
        let found = match tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await {
            Ok(Ok(found)) => found,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err("the DNS lookup timed out".into()),
        };
        let mut addresses: Vec<IpAddr> = Vec::new();
        for ip in found.map(|a| a.ip()) {
            if !addresses.contains(&ip) {
                addresses.push(ip);
            }
        }
        if addresses.is_empty() { Err("no address".into()) } else { Ok(addresses) }
    }

    async fn get(&self, host: &str, addr: SocketAddr, path: &str) -> std::result::Result<String, ProbeError> {
        // The name goes in the request; the connection goes to the address
        // that was resolved, so both checks talk about the same answer.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .resolve(host, addr)
            .build()
            .map_err(|e| ProbeError::Other(e.to_string()))?;
        let url = format!("http://{host}:{}{path}", addr.port());
        let resp = client.get(url).send().await.map_err(|e| {
            if e.is_timeout() {
                ProbeError::TimedOut
            } else if e.is_connect() {
                // The cause, without reqwest's wrapping ("error sending request for url…").
                let mut cause: &dyn std::error::Error = &e;
                while let Some(next) = cause.source() {
                    cause = next;
                }
                ProbeError::Connect(cause.to_string())
            } else {
                ProbeError::Other(e.to_string())
            }
        })?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(ProbeError::OtherServer(status));
        }
        // An answer of this server is a few dozen bytes: read no more of
        // whatever page another server sends.
        let mut resp = resp;
        let mut body: Vec<u8> = Vec::new();
        while body.len() < 256 {
            match resp.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(e) => return Err(ProbeError::Other(e.to_string())),
            }
        }
        body.truncate(256);
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// The IPv4 address only: the address of an interface says which IPv6
    /// address the machine *leaves* with, and that is often a temporary one
    /// (RFC 8981) no DNS record should hold. An IPv6 address is `--public-ip`'s.
    async fn public_addresses(&self) -> Vec<IpAddr> {
        let somewhere = SocketAddr::from(([1, 1, 1, 1], 53));
        // A machine with a public address on its interface needs to ask nobody.
        match outbound_address(somewhere).await.filter(|ip| is_public(*ip)) {
            Some(ip) => vec![ip],
            // Behind NAT: only someone outside sees the public address.
            None => echoed_address().await.into_iter().collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use ferry_core::DomainSource;

    use super::*;

    /// A network where names under a domain resolve to fixed addresses, and
    /// each address answers in a fixed way.
    #[derive(Default)]
    pub(crate) struct FakeNet {
        /// Domain → the addresses every name under it resolves to.
        pub dns: StdMutex<HashMap<String, Vec<IpAddr>>>,
        /// Address → what a verification request gets there: `Ok(probe id)`
        /// of the Ferry server that answers, or the error.
        pub http: StdMutex<HashMap<IpAddr, std::result::Result<String, ProbeError>>>,
        pub public: StdMutex<Vec<IpAddr>>,
        pub requests: StdMutex<Vec<(String, SocketAddr)>>,
    }

    pub(crate) fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    impl FakeNet {
        pub(crate) fn point(&self, domain: &str, addresses: &[&str]) {
            lock(&self.dns).insert(domain.to_string(), addresses.iter().map(|a| ip(a)).collect());
        }

        pub(crate) fn answer(&self, address: &str, with: std::result::Result<&str, ProbeError>) {
            lock(&self.http).insert(ip(address), with.map(str::to_string));
        }
    }

    #[async_trait]
    impl Network for FakeNet {
        async fn resolve(&self, host: &str) -> std::result::Result<Vec<IpAddr>, String> {
            let dns = lock(&self.dns);
            dns.iter()
                .find(|(domain, _)| host.ends_with(&format!(".{domain}")))
                .map(|(_, addresses)| addresses.clone())
                .ok_or_else(|| "nodename nor servname provided, or not known".to_string())
        }

        async fn get(&self, host: &str, addr: SocketAddr, path: &str) -> std::result::Result<String, ProbeError> {
            lock(&self.requests).push((host.to_string(), addr));
            let token = path.strip_prefix(PROBE_PATH).unwrap_or_default();
            match lock(&self.http).get(&addr.ip()) {
                Some(Ok(probe_id)) => Ok(probe_answer(token, probe_id)),
                Some(Err(e)) => Err(e.clone()),
                None => Err(ProbeError::Connect("connection refused".into())),
            }
        }

        async fn public_addresses(&self) -> Vec<IpAddr> {
            lock(&self.public).clone()
        }
    }

    // Next to the documentation ranges, not in them: `is_public` knows those.
    const HERE: &str = "203.0.114.10";
    const ELSEWHERE: &str = "198.51.101.7";

    fn outcomes(o: &Outcome) -> (bool, CheckOutcome, CheckOutcome) {
        (o.ok, o.checks[0].outcome, o.checks[1].outcome)
    }

    async fn run(net: &FakeNet, expected: &[&str]) -> Outcome {
        let expected: Vec<IpAddr> = expected.iter().map(|a| ip(a)).collect();
        check(net, "example.com", 80, "me", &expected).await
    }

    #[tokio::test]
    async fn a_domain_without_a_record_waits() {
        let net = FakeNet::default();
        let o = run(&net, &[HERE]).await;
        assert_eq!(outcomes(&o), (false, CheckOutcome::Failed, CheckOutcome::Skipped));
        assert!(o.checks[0].message.contains("*.example.com"), "{}", o.checks[0].message);
        assert!(lock(&net.requests).is_empty());
    }

    #[tokio::test]
    async fn a_domain_that_points_here_and_answers_is_verified() {
        let net = FakeNet::default();
        net.point("example.com", &[HERE]);
        net.answer(HERE, Ok("me"));
        let o = run(&net, &[HERE]).await;
        assert_eq!(outcomes(&o), (true, CheckOutcome::Passed, CheckOutcome::Passed));
        assert_eq!(o.checks[0].message, "*.example.com resolves to 203.0.114.10, this server.");
        // The request went to a made-up name under the domain, on the public port.
        let requests = lock(&net.requests).clone();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].0.starts_with("ferry-check-") && requests[0].0.ends_with(".example.com"));
        assert_eq!(requests[0].1, SocketAddr::new(ip(HERE), 80));
        // Without knowing its own address, the answer alone decides.
        assert_eq!(outcomes(&run(&net, &[]).await), (true, CheckOutcome::Passed, CheckOutcome::Passed));
    }

    #[tokio::test]
    async fn a_domain_that_points_elsewhere_is_not() {
        let net = FakeNet::default();
        net.point("example.com", &[ELSEWHERE]);
        // A parked page, another Ferry server, nothing at all, a timeout.
        for answer in [Err(ProbeError::OtherServer(404)), Ok("someone-else"), Err(ProbeError::TimedOut)] {
            net.answer(ELSEWHERE, answer);
            let o = run(&net, &[HERE]).await;
            assert_eq!(outcomes(&o), (false, CheckOutcome::Failed, CheckOutcome::Failed), "{o:?}");
            assert!(o.checks[0].message.contains("Point the record at 203.0.114.10"), "{}", o.checks[0].message);
        }
        lock(&net.http).clear();
        let o = run(&net, &[HERE]).await;
        assert!(o.checks[1].message.contains("connection refused"), "{}", o.checks[1].message);
        // Its own address unknown: it can't say the record is wrong, only that nothing answered.
        let o = run(&net, &[]).await;
        assert_eq!(outcomes(&o), (false, CheckOutcome::Warning, CheckOutcome::Failed));
    }

    #[tokio::test]
    async fn a_server_that_cannot_reach_itself_trusts_the_dns() {
        let net = FakeNet::default();
        net.point("example.com", &[HERE]);
        net.answer(HERE, Err(ProbeError::TimedOut));
        let o = run(&net, &[HERE]).await;
        assert_eq!(outcomes(&o), (true, CheckOutcome::Passed, CheckOutcome::Warning));
        assert!(o.checks[1].message.contains("could not reach itself"), "{}", o.checks[1].message);
        // Someone else answering at "its" address is not that.
        net.answer(HERE, Err(ProbeError::OtherServer(502)));
        assert_eq!(outcomes(&run(&net, &[HERE]).await), (false, CheckOutcome::Passed, CheckOutcome::Failed));
    }

    #[tokio::test]
    async fn a_domain_behind_a_cdn_works_with_a_warning() {
        let net = FakeNet::default();
        net.point("example.com", &["104.16.0.1"]);
        net.answer("104.16.0.1", Ok("me"));
        let o = run(&net, &[HERE]).await;
        assert_eq!(outcomes(&o), (true, CheckOutcome::Warning, CheckOutcome::Passed));
        assert!(o.checks[0].message.contains("CDN"), "{}", o.checks[0].message);
    }

    #[tokio::test]
    async fn addresses_are_asked_until_this_server_answers() {
        let net = FakeNet::default();
        net.point("example.com", &["2001:db8::1", ELSEWHERE, HERE]);
        net.answer(HERE, Ok("me"));
        net.answer(ELSEWHERE, Err(ProbeError::TimedOut));
        let o = run(&net, &[HERE]).await;
        // It answers, but a record points elsewhere: part of the visitors go there.
        assert_eq!(outcomes(&o), (true, CheckOutcome::Warning, CheckOutcome::Passed));
        let asked: Vec<IpAddr> = lock(&net.requests).iter().map(|r| r.1.ip()).collect();
        assert_eq!(asked, vec![ip(ELSEWHERE), ip(HERE)]);
    }

    #[tokio::test]
    async fn a_private_address_works_from_here_only() {
        let net = FakeNet::default();
        net.point("lvh.me", &["127.0.0.1"]);
        net.answer("127.0.0.1", Ok("me"));
        for expected in [vec![ip(HERE)], vec![]] {
            let o = check(&net, "lvh.me", 8080, "me", &expected).await;
            assert_eq!(outcomes(&o), (true, CheckOutcome::Warning, CheckOutcome::Passed));
            assert!(o.checks[0].message.contains("private address"), "{}", o.checks[0].message);
            assert!(o.checks[1].message.contains("port 8080"), "{}", o.checks[1].message);
        }
    }

    #[tokio::test]
    async fn an_ipv6_record_is_not_judged_without_the_servers_ipv6_address() {
        let net = FakeNet::default();
        net.point("example.com", &["2a01:4f8::1", HERE]);
        net.answer(HERE, Ok("me"));
        let o = run(&net, &[HERE]).await;
        assert_eq!(outcomes(&o), (true, CheckOutcome::Passed, CheckOutcome::Passed));
        assert!(o.checks[0].message.contains("203.0.114.10, this server."), "{}", o.checks[0].message);
        assert!(o.checks[0].message.contains("2a01:4f8::1, which was not compared"), "{}", o.checks[0].message);
        // Known (--public-ip): it must be this server's too.
        let o = run(&net, &[HERE, "2a01:4f8::2"]).await;
        assert_eq!(outcomes(&o), (true, CheckOutcome::Warning, CheckOutcome::Passed));
        assert!(o.checks[0].message.contains("only 203.0.114.10 is this server"), "{}", o.checks[0].message);
        let o = run(&net, &[HERE, "2a01:4f8::1"]).await;
        assert_eq!(outcomes(&o), (true, CheckOutcome::Passed, CheckOutcome::Passed));
        // Only an IPv6 record, and no IPv6 address to compare it with: the answer decides.
        net.point("example.com", &["2a01:4f8::1"]);
        net.answer("2a01:4f8::1", Ok("me"));
        assert_eq!(outcomes(&run(&net, &[HERE]).await), (true, CheckOutcome::Passed, CheckOutcome::Passed));
    }

    #[test]
    fn statuses_follow_the_outcomes() {
        let ok = || Outcome { ok: true, checks: vec![] };
        let failed =
            || Outcome { ok: false, checks: vec![DomainCheck::new(DomainCheckKind::Dns, CheckOutcome::Failed, "x")] };
        let now = ferry_core::now();
        let mut d = Domain::new("example.com", DomainSource::Connected);
        // Pending until it works, however often it fails.
        for _ in 0..5 {
            apply(&mut d, failed(), now);
        }
        assert_eq!((d.status, d.failures, d.verified_at), (DomainStatus::Pending, 5, None));
        assert_eq!(d.checks.len(), 1);
        apply(&mut d, ok(), now);
        assert_eq!(
            (d.status, d.failures, d.verified_at, d.checked_at),
            (DomainStatus::Active, 0, Some(now), Some(now))
        );
        // One or two failures are not a misconfiguration; three in a row are.
        apply(&mut d, failed(), now);
        apply(&mut d, failed(), now);
        assert_eq!(d.status, DomainStatus::Active);
        apply(&mut d, ok(), now);
        assert_eq!(d.failures, 0);
        for _ in 0..3 {
            apply(&mut d, failed(), now);
        }
        assert_eq!(d.status, DomainStatus::Misconfigured);
        assert!(d.is_served());
        apply(&mut d, ok(), now);
        assert_eq!(d.status, DomainStatus::Active);
    }

    #[test]
    fn schedule() {
        let now = ferry_core::now();
        let mut d = Domain::new("example.com", DomainSource::Connected);
        assert!(due(&d, now));
        d.checked_at = Some(now - chrono::Duration::seconds(10));
        assert!(!due(&d, now));
        d.checked_at = Some(now - chrono::Duration::seconds(16));
        assert!(due(&d, now));
        // An old pending domain is asked less often.
        d.created_at = now - chrono::Duration::hours(2);
        assert!(!due(&d, now));
        d.checked_at = Some(now - chrono::Duration::seconds(61));
        assert!(due(&d, now));
        d.status = DomainStatus::Active;
        assert!(!due(&d, now));
        d.checked_at = Some(now - chrono::Duration::minutes(11));
        assert!(due(&d, now));
        // A local name is never verified.
        let mut local = Domain::new("dev.localhost", DomainSource::Connected);
        local.checked_at = None;
        assert!(!due(&local, now));
    }

    #[test]
    fn public_addresses_only() {
        for public in ["203.0.114.10", "8.8.8.8", "2a01:4f8::1"] {
            assert!(is_public(ip(public)), "{public}");
        }
        for private in [
            "10.0.0.5",
            "192.168.1.20",
            "172.16.3.4",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "198.18.0.1",
            "192.0.2.1",
            "240.0.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip(private)), "{private}");
        }
        assert_eq!(list(&[ip("1.1.1.1")]), "1.1.1.1");
        assert_eq!(list(&[ip("1.1.1.1"), ip("2.2.2.2"), ip("::1")]), "1.1.1.1, 2.2.2.2 and ::1");
    }
}
