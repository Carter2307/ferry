//! The host → upstreams routing table shared between the engine (writer) and
//! the proxy (reader).

use std::borrow::Cow;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

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

/// Normalize a hostname or `Host` header value for routing: trims
/// whitespace, strips a `:port` suffix (bracketed IPv6 literals such as
/// `[::1]:8080` become `::1`), strips trailing dots and lowercases ASCII.
///
/// `"Web.Example.COM.:8080"` → `"web.example.com"`.
pub fn normalize_host(host: &str) -> String {
    normalize(host).into_owned()
}

/// Allocation-free when the input is already normalized (the common case).
fn normalize(host: &str) -> Cow<'_, str> {
    let s = host.trim();
    let name = if let Some(rest) = s.strip_prefix('[') {
        // Bracketed IPv6 literal, optionally followed by `:port`.
        match rest.find(']') {
            Some(end) => &rest[..end],
            None => rest,
        }
    } else if s.bytes().filter(|&b| b == b':').count() > 1 {
        // Bare IPv6 literal (no brackets means no port).
        s
    } else {
        match s.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
            _ => s,
        }
    };
    let name = name.trim_end_matches('.');
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(name.to_ascii_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

/// Routing state of one service. Every host of the service points at the same
/// `Arc`, so round-robin is per service rather than per hostname.
#[derive(Debug)]
struct ServiceRoute {
    service_id: String,
    upstreams: Vec<SocketAddr>,
    suspended: bool,
    /// Round-robin cursor (wrapping).
    next: AtomicUsize,
}

impl ServiceRoute {
    /// Round-robin pick plus, when the service has another distinct upstream,
    /// the one after it (used to retry failed connects elsewhere).
    fn pick(&self) -> Option<(SocketAddr, Option<SocketAddr>)> {
        let len = self.upstreams.len();
        if len == 0 {
            return None;
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % len;
        let chosen = self.upstreams[i];
        let fallback = (1..len).map(|k| self.upstreams[(i + k) % len]).find(|addr| *addr != chosen);
        Some((chosen, fallback))
    }
}

#[derive(Debug, Default)]
struct Table {
    /// Normalized host → route of the owning service.
    by_host: HashMap<String, Arc<ServiceRoute>>,
    /// Service id → the normalized hosts it currently owns (never empty).
    by_service: HashMap<String, Vec<String>>,
}

impl Table {
    /// Current round-robin position of a service, so that periodic route
    /// refreshes with identical upstreams don't reset the rotation.
    fn cursor_of(&self, service_id: &str) -> usize {
        self.by_service
            .get(service_id)
            .and_then(|hosts| hosts.first())
            .and_then(|host| self.by_host.get(host))
            .map_or(0, |route| route.next.load(Ordering::Relaxed))
    }

    /// Drop every host owned by `service_id`.
    fn remove_service(&mut self, service_id: &str) {
        if let Some(hosts) = self.by_service.remove(service_id) {
            for host in hosts {
                if self.by_host.get(&host).is_some_and(|r| r.service_id == service_id) {
                    self.by_host.remove(&host);
                }
            }
        }
    }

    /// Replace the hosts of `route.service_id` with `hosts` (already
    /// normalized and deduplicated). Hosts owned by another service are taken
    /// over (last writer wins) and removed from that service's index.
    fn install(&mut self, hosts: Vec<String>, route: ServiceRoute) {
        let service_id = route.service_id.clone();
        self.remove_service(&service_id);
        if hosts.is_empty() {
            return;
        }
        let route = Arc::new(route);
        for host in &hosts {
            let Some(previous) = self.by_host.insert(host.clone(), route.clone()) else { continue };
            if previous.service_id == service_id {
                continue;
            }
            tracing::warn!(
                host = %host,
                from = %previous.service_id,
                to = %service_id,
                "proxy: hostname was routed to another service; re-routing it"
            );
            if let Some(owned) = self.by_service.get_mut(&previous.service_id) {
                owned.retain(|h| h != host);
                if owned.is_empty() {
                    self.by_service.remove(&previous.service_id);
                }
            }
        }
        self.by_service.insert(service_id, hosts);
    }
}

/// Normalize, drop empties and deduplicate (keeping the first occurrence).
fn normalize_all(hosts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(hosts.len());
    for host in hosts {
        let host = normalize(host);
        if !host.is_empty() && !out.iter().any(|h| h == host.as_ref()) {
            out.push(host.into_owned());
        }
    }
    out
}

/// Shared, thread-safe host → upstreams table. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    inner: Arc<RwLock<Table>>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self::default()
    }

    // A panic while holding the lock can't leave the table half-updated in a
    // way that matters more than refusing to route, so poisoning is ignored.
    fn read(&self) -> RwLockReadGuard<'_, Table> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Table> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Atomically make `hosts` (case-insensitive, port ignored) point at
    /// `upstreams` for `service_id`, removing any other host previously
    /// routed to that service. An empty `upstreams` yields `NoUpstreams`.
    pub fn set_service_routes(&self, service_id: &str, hosts: &[String], upstreams: Vec<SocketAddr>) {
        let hosts = normalize_all(hosts);
        let mut table = self.write();
        let cursor = table.cursor_of(service_id);
        table.install(
            hosts,
            ServiceRoute {
                service_id: service_id.to_string(),
                upstreams,
                suspended: false,
                next: AtomicUsize::new(cursor),
            },
        );
    }

    /// Route `hosts` of `service_id` to the "suspended" page.
    pub fn set_service_suspended(&self, service_id: &str, hosts: &[String]) {
        let hosts = normalize_all(hosts);
        self.write().install(
            hosts,
            ServiceRoute {
                service_id: service_id.to_string(),
                upstreams: Vec::new(),
                suspended: true,
                next: AtomicUsize::new(0),
            },
        );
    }

    /// Remove every route of a service.
    pub fn remove_service(&self, service_id: &str) {
        self.write().remove_service(service_id);
    }

    /// Look up a `Host` header value (may include `:port`; case-insensitive).
    pub fn resolve(&self, host: &str) -> Resolution {
        self.resolve_with_fallback(host).0
    }

    /// Like [`resolve`](Self::resolve), plus another upstream of the same
    /// service (if it has one) to retry on when connecting to the first fails.
    pub(crate) fn resolve_with_fallback(&self, host: &str) -> (Resolution, Option<SocketAddr>) {
        let key = normalize(host);
        let table = self.read();
        let Some(route) = table.by_host.get(key.as_ref()) else {
            return (Resolution::NotFound, None);
        };
        if route.suspended {
            return (Resolution::Suspended, None);
        }
        match route.pick() {
            Some((upstream, fallback)) => (Resolution::Upstream(upstream), fallback),
            None => (Resolution::NoUpstreams, None),
        }
    }

    /// All routed hostnames (sorted) — used by the TLS manager.
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self.read().by_host.keys().cloned().collect();
        hosts.sort();
        hosts
    }

    /// Current routes (sorted by host), for debugging / status.
    pub fn snapshot(&self) -> Vec<RouteSnapshot> {
        let mut routes: Vec<RouteSnapshot> = self
            .read()
            .by_host
            .iter()
            .map(|(host, route)| RouteSnapshot {
                host: host.clone(),
                service_id: route.service_id.clone(),
                upstreams: route.upstreams.clone(),
                suspended: route.suspended,
            })
            .collect();
        routes.sort_by(|a, b| a.host.cmp(&b.host));
        routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalizes_hosts() {
        assert_eq!(normalize_host("Web.Localhost"), "web.localhost");
        assert_eq!(normalize_host("web.localhost:8080"), "web.localhost");
        assert_eq!(normalize_host("web.localhost.:8080"), "web.localhost");
        assert_eq!(normalize_host("  App.Example.com.  "), "app.example.com");
        assert_eq!(normalize_host("web.localhost:"), "web.localhost");
        assert_eq!(normalize_host("[::1]:8080"), "::1");
        assert_eq!(normalize_host("[::1]"), "::1");
        assert_eq!(normalize_host("[FE80::1]"), "fe80::1");
        assert_eq!(normalize_host("::1"), "::1");
        assert_eq!(normalize_host("127.0.0.1:80"), "127.0.0.1");
        assert_eq!(normalize_host("[::1"), "::1");
        assert_eq!(normalize_host("host:abc"), "host:abc");
        assert_eq!(normalize_host(""), "");
        assert!(matches!(normalize("already.normal"), Cow::Borrowed(_)));
    }

    #[test]
    fn resolves_case_and_port_insensitively() {
        let t = RouteTable::new();
        t.set_service_routes("srv-1", &hosts(&["Web.Localhost", "app.example.com."]), vec![addr(1000)]);
        for h in ["web.localhost", "WEB.localhost:8080", "web.localhost.", "App.Example.Com:443", "app.example.com"] {
            assert_eq!(t.resolve(h), Resolution::Upstream(addr(1000)), "{h}");
        }
        assert_eq!(t.resolve("other.localhost"), Resolution::NotFound);
        assert_eq!(t.resolve(""), Resolution::NotFound);
        assert_eq!(t.hosts(), vec!["app.example.com", "web.localhost"]);
    }

    #[test]
    fn round_robins_across_upstreams() {
        let t = RouteTable::new();
        t.set_service_routes("srv-1", &hosts(&["a.test", "b.test"]), vec![addr(1), addr(2), addr(3)]);
        let picks: Vec<_> = (0..6)
            .map(|i| match t.resolve(if i % 2 == 0 { "a.test" } else { "b.test" }) {
                Resolution::Upstream(a) => a.port(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        // One cursor per service, shared by all of its hosts.
        assert_eq!(picks, vec![1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn refresh_keeps_round_robin_position() {
        let t = RouteTable::new();
        t.set_service_routes("srv-1", &hosts(&["a.test"]), vec![addr(1), addr(2)]);
        assert_eq!(t.resolve("a.test"), Resolution::Upstream(addr(1)));
        t.set_service_routes("srv-1", &hosts(&["a.test"]), vec![addr(1), addr(2)]);
        assert_eq!(t.resolve("a.test"), Resolution::Upstream(addr(2)));
    }

    #[test]
    fn fallback_is_another_distinct_upstream() {
        let t = RouteTable::new();
        t.set_service_routes("one", &hosts(&["one.test"]), vec![addr(1)]);
        assert_eq!(t.resolve_with_fallback("one.test"), (Resolution::Upstream(addr(1)), None));
        t.set_service_routes("dup", &hosts(&["dup.test"]), vec![addr(1), addr(1)]);
        assert_eq!(t.resolve_with_fallback("dup.test"), (Resolution::Upstream(addr(1)), None));
        t.set_service_routes("two", &hosts(&["two.test"]), vec![addr(1), addr(2)]);
        assert_eq!(t.resolve_with_fallback("two.test"), (Resolution::Upstream(addr(1)), Some(addr(2))));
        assert_eq!(t.resolve_with_fallback("two.test"), (Resolution::Upstream(addr(2)), Some(addr(1))));
    }

    #[test]
    fn set_service_routes_replaces_hosts_atomically() {
        let t = RouteTable::new();
        t.set_service_routes("srv-1", &hosts(&["old.test", "keep.test"]), vec![addr(1)]);
        t.set_service_routes("srv-2", &hosts(&["other.test"]), vec![addr(9)]);
        t.set_service_routes("srv-1", &hosts(&["keep.test", "new.test", "NEW.test"]), vec![addr(2)]);
        assert_eq!(t.resolve("old.test"), Resolution::NotFound);
        assert_eq!(t.resolve("keep.test"), Resolution::Upstream(addr(2)));
        assert_eq!(t.resolve("new.test"), Resolution::Upstream(addr(2)));
        assert_eq!(t.resolve("other.test"), Resolution::Upstream(addr(9)));
        assert_eq!(
            t.snapshot(),
            vec![
                RouteSnapshot {
                    host: "keep.test".into(),
                    service_id: "srv-1".into(),
                    upstreams: vec![addr(2)],
                    suspended: false
                },
                RouteSnapshot {
                    host: "new.test".into(),
                    service_id: "srv-1".into(),
                    upstreams: vec![addr(2)],
                    suspended: false
                },
                RouteSnapshot {
                    host: "other.test".into(),
                    service_id: "srv-2".into(),
                    upstreams: vec![addr(9)],
                    suspended: false
                },
            ]
        );
        // No hosts at all → the service disappears from the table.
        t.set_service_routes("srv-1", &[], vec![addr(2)]);
        assert_eq!(t.hosts(), vec!["other.test"]);
    }

    #[test]
    fn empty_upstreams_and_suspension() {
        let t = RouteTable::new();
        t.set_service_routes("srv-1", &hosts(&["web.test"]), vec![]);
        assert_eq!(t.resolve("web.test"), Resolution::NoUpstreams);
        t.set_service_suspended("srv-1", &hosts(&["web.test", "alias.test"]));
        assert_eq!(t.resolve("web.test"), Resolution::Suspended);
        assert_eq!(t.resolve("alias.test:80"), Resolution::Suspended);
        assert!(t.snapshot().iter().all(|r| r.suspended && r.upstreams.is_empty()));
        t.set_service_routes("srv-1", &hosts(&["web.test"]), vec![addr(3)]);
        assert_eq!(t.resolve("web.test"), Resolution::Upstream(addr(3)));
        assert_eq!(t.resolve("alias.test"), Resolution::NotFound);
    }

    #[test]
    fn host_takeover_updates_both_services() {
        let t = RouteTable::new();
        t.set_service_routes("a", &hosts(&["shared.test", "a.test"]), vec![addr(1)]);
        t.set_service_routes("b", &hosts(&["shared.test"]), vec![addr(2)]);
        assert_eq!(t.resolve("shared.test"), Resolution::Upstream(addr(2)));
        assert_eq!(t.resolve("a.test"), Resolution::Upstream(addr(1)));
        // Removing `a` must not remove the host `b` now owns.
        t.remove_service("a");
        assert_eq!(t.resolve("shared.test"), Resolution::Upstream(addr(2)));
        assert_eq!(t.resolve("a.test"), Resolution::NotFound);
        t.remove_service("b");
        assert!(t.hosts().is_empty());
        assert!(t.snapshot().is_empty());
        t.remove_service("never-existed");
    }

    #[test]
    fn clones_share_state_and_survive_poisoning() {
        let t = RouteTable::new();
        let t2 = t.clone();
        t2.set_service_routes("srv", &hosts(&["x.test"]), vec![addr(7)]);
        let poison = t.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poison.inner.write().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(t.inner.is_poisoned());
        assert_eq!(t.resolve("x.test"), Resolution::Upstream(addr(7)));
        t.remove_service("srv");
        assert!(t2.hosts().is_empty());
    }

    #[test]
    fn concurrent_resolves_see_consistent_routes() {
        let t = RouteTable::new();
        t.set_service_routes("srv", &hosts(&["c.test"]), vec![addr(1), addr(2)]);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..1000 {
                        match t.resolve("c.test") {
                            Resolution::Upstream(a) => assert!(a.port() <= 4),
                            other => panic!("unexpected {other:?}"),
                        }
                    }
                });
            }
            s.spawn(|| {
                for i in 0..200u16 {
                    t.set_service_routes("srv", &hosts(&["c.test"]), vec![addr(1 + i % 2), addr(3 + i % 2)]);
                }
            });
        });
    }
}
