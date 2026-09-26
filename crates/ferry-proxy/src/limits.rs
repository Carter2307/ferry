//! Client connection admission and lifetime: a global cap and a per-address
//! cap on open connections, and per-connection idle tracking so connections
//! that send nothing (or stop sending requests) are closed.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::Instant;

/// Upper bound (and fallback) for [`ConnectionLimits::max_connections`].
const DEFAULT_MAX_CONNECTIONS: usize = 10_000;
/// Lower bound of the connection limit derived from the open-file limit.
const MIN_AUTO_MAX_CONNECTIONS: usize = 32;
/// Hysteresis: the "at the limit" state ends once this fraction of the
/// connection slots is free again.
const PRESSURE_RELEASE_DIVISOR: usize = 10;
/// Repeated limit warnings are logged at most this often.
const WARN_INTERVAL: Duration = Duration::from_secs(60);

/// Limits and timeouts of the proxy's client connections (both listeners).
///
/// The defaults suit a public edge; see each field.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConnectionLimits {
    /// Most client connections open at once across both listeners, websocket
    /// tunnels included. At the limit, new connections wait in the kernel's
    /// listen backlog, and connections with no request in flight are closed
    /// after [`overload_idle_timeout`](Self::overload_idle_timeout) to make
    /// room. `None` (default): derived from the process's open-file limit —
    /// each proxied connection may also hold an upstream socket, and the rest
    /// of the process needs descriptors too — capped at 10 000.
    pub max_connections: Option<usize>,
    /// Most connections open at once from one client address (IPv6: one
    /// /64). Further connections from that address are closed as soon as
    /// they are accepted. Loopback clients (a local front proxy or tunnel)
    /// are exempt. `0` disables the check. Default 256.
    pub max_connections_per_ip: usize,
    /// A TLS handshake must complete within this. Default 10 s.
    pub tls_handshake_timeout: Duration,
    /// A new connection must deliver a complete first request head within
    /// this (whatever the protocol turns out to be), and an HTTP/1 connection
    /// must deliver each following request head within this once the previous
    /// exchange is done — which also bounds idle HTTP/1 keep-alive. Default 30 s.
    pub header_read_timeout: Duration,
    /// A connection with no request in flight is closed after this (the
    /// idle limit of HTTP/2 connections; HTTP/1 ones are closed sooner by
    /// `header_read_timeout`). Requests in flight — long SSE responses,
    /// slow uploads — are never cut by these timeouts (a request body that
    /// stalls is: see `request_body_timeout`). Default 60 s.
    pub idle_timeout: Duration,
    /// Replaces the two timeouts above for connections with no request in
    /// flight while the connection limit is reached. Default 5 s.
    pub overload_idle_timeout: Duration,
    /// Longest wait for the next piece of a request body (like nginx's
    /// `client_body_timeout`): a client that stops sending mid-body gets a
    /// `408` (or, once the upstream has started answering, the exchange is
    /// aborted), so a body trickled byte by byte cannot hold a connection
    /// and an upstream connection forever. Slow uploads are fine as long as
    /// data keeps coming; time the upstream takes to read the body does not
    /// count, and responses (SSE, long downloads) and websocket tunnels are
    /// not affected. Default 60 s.
    pub request_body_timeout: Duration,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        ConnectionLimits {
            max_connections: None,
            max_connections_per_ip: 256,
            tls_handshake_timeout: Duration::from_secs(10),
            header_read_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            overload_idle_timeout: Duration::from_secs(5),
            request_body_timeout: Duration::from_secs(60),
        }
    }
}

impl ConnectionLimits {
    /// [`max_connections`](Self::max_connections), resolved.
    pub(crate) fn effective_max_connections(&self) -> usize {
        match self.max_connections {
            Some(max) => max.max(1),
            None => auto_max_connections(open_file_limit()),
        }
    }
}

/// Connection limit for a process allowed `nofile` open descriptors: a
/// quarter of them (at most 1024) is left to the rest of the process, and
/// each client connection may pair with an upstream connection.
fn auto_max_connections(nofile: Option<usize>) -> usize {
    let Some(limit) = nofile else { return DEFAULT_MAX_CONNECTIONS };
    let reserve = (limit / 4).min(1024);
    ((limit - reserve) / 2).clamp(MIN_AUTO_MAX_CONNECTIONS, DEFAULT_MAX_CONNECTIONS)
}

/// The soft `RLIMIT_NOFILE`, if finite.
#[cfg(unix)]
fn open_file_limit() -> Option<usize> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit(2) only writes the struct passed by pointer, which
    // is valid and properly aligned for the whole call.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    if rc != 0 || limit.rlim_cur == libc::RLIM_INFINITY {
        return None;
    }
    usize::try_from(limit.rlim_cur).ok()
}

#[cfg(not(unix))]
fn open_file_limit() -> Option<usize> {
    None
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Logs a warning at most once per [`WARN_INTERVAL`].
#[derive(Default)]
struct Throttle(Mutex<Option<Instant>>);

impl Throttle {
    fn ready(&self) -> bool {
        let mut last = lock(&self.0);
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < WARN_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

// ------------------------------------------------------------ admission ----

/// Per-address accounting key: IPv4 address, or the /64 of an IPv6 one (a
/// single client typically controls a whole /64).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AddrKey {
    V4(Ipv4Addr),
    V6Prefix(u64),
}

/// `None` for addresses exempt from the per-address limit (loopback).
fn addr_key(ip: IpAddr) -> Option<AddrKey> {
    match ip.to_canonical() {
        ip if ip.is_loopback() => None,
        IpAddr::V4(v4) => Some(AddrKey::V4(v4)),
        IpAddr::V6(v6) => Some(AddrKey::V6Prefix((v6.to_bits() >> 64) as u64)),
    }
}

/// Shared by every listener of one `serve` call.
pub(crate) struct ConnLimiter {
    max: usize,
    per_ip_max: usize,
    slots: Arc<Semaphore>,
    per_ip: Mutex<HashMap<AddrKey, usize>>,
    /// `true` while the connection limit is reached (with hysteresis).
    pressure: watch::Sender<bool>,
    limit_warning: Throttle,
    per_ip_warning: Throttle,
}

impl ConnLimiter {
    pub(crate) fn new(limits: &ConnectionLimits) -> Arc<Self> {
        let max = limits.effective_max_connections();
        Arc::new(ConnLimiter {
            max,
            per_ip_max: limits.max_connections_per_ip,
            slots: Arc::new(Semaphore::new(max)),
            per_ip: Mutex::new(HashMap::new()),
            pressure: watch::Sender::new(false),
            limit_warning: Throttle::default(),
            per_ip_warning: Throttle::default(),
        })
    }

    pub(crate) fn max_connections(&self) -> usize {
        self.max
    }

    pub(crate) fn max_connections_per_ip(&self) -> usize {
        self.per_ip_max
    }

    /// Follows the "connection limit reached" state.
    pub(crate) fn pressure(&self) -> watch::Receiver<bool> {
        self.pressure.subscribe()
    }

    /// A free connection slot, waiting for one at the limit. Accept loops
    /// call this *before* accepting, so that at the limit new connections
    /// queue in the kernel's backlog instead of using descriptors.
    pub(crate) async fn reserve(&self) -> OwnedSemaphorePermit {
        if let Ok(permit) = self.slots.clone().try_acquire_owned() {
            return permit;
        }
        let already = self.pressure.send_replace(true);
        if !already && self.limit_warning.ready() {
            tracing::warn!(
                "proxy: {} client connections open (the limit); new connections wait and idle ones are closed early",
                self.max
            );
        }
        let permit = self.slots.clone().acquire_owned().await.expect("the connection semaphore is never closed");
        // Slots may have been freed between the failed attempt and the
        // "limit reached" signal above: don't leave it on for nothing.
        self.relieve();
        permit
    }

    /// Admit a connection accepted from `ip` with a slot from
    /// [`reserve`](Self::reserve); `None` when that address already has
    /// [`ConnectionLimits::max_connections_per_ip`] connections open.
    pub(crate) fn admit(self: &Arc<Self>, slot: OwnedSemaphorePermit, ip: IpAddr) -> Option<ConnSlot> {
        let key = if self.per_ip_max == 0 { None } else { addr_key(ip) };
        if let Some(key) = key {
            let mut per_ip = lock(&self.per_ip);
            let count = per_ip.entry(key).or_insert(0);
            if *count >= self.per_ip_max {
                drop(per_ip);
                if self.per_ip_warning.ready() {
                    tracing::warn!(
                        client = %ip,
                        "proxy: client address has {} connections open (the per-address limit); refusing more",
                        self.per_ip_max
                    );
                }
                return None;
            }
            *count += 1;
        }
        Some(ConnSlot { limiter: self.clone(), key, permit: Some(slot) })
    }

    fn release(&self, key: Option<AddrKey>) {
        if let Some(key) = key {
            let mut per_ip = lock(&self.per_ip);
            if let Some(count) = per_ip.get_mut(&key) {
                *count -= 1;
                if *count == 0 {
                    per_ip.remove(&key);
                }
            }
        }
    }

    /// Called after a slot was returned: leave the "limit reached" state
    /// once enough slots are free.
    fn relieve(&self) {
        if *self.pressure.borrow() && self.slots.available_permits() >= (self.max / PRESSURE_RELEASE_DIVISOR).max(1) {
            self.pressure.send_replace(false);
        }
    }
}

/// One admitted client connection: its global slot and per-address count
/// are released on drop. Websocket tunnels keep it until they close.
pub(crate) struct ConnSlot {
    limiter: Arc<ConnLimiter>,
    key: Option<AddrKey>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        self.limiter.release(self.key);
        drop(self.permit.take());
        self.limiter.relieve();
    }
}

// -------------------------------------------------------------- activity ----

/// Requests in flight on one client connection, to find when it is idle.
pub(crate) struct Activity {
    state: Mutex<ActivityState>,
    changed: Notify,
}

struct ActivityState {
    in_flight: usize,
    /// Requests started so far.
    started: u64,
    /// When `in_flight` last dropped to zero (or the connection was set up).
    idle_since: Instant,
}

/// Held from the moment hyper dispatches a request until its response body
/// has been sent (or dropped).
pub(crate) struct ActiveRequest(Arc<Activity>);

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.in_flight -= 1;
        if state.in_flight == 0 {
            state.idle_since = Instant::now();
        }
        drop(state);
        self.0.changed.notify_one();
    }
}

impl Activity {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Activity {
            state: Mutex::new(ActivityState { in_flight: 0, started: 0, idle_since: Instant::now() }),
            changed: Notify::new(),
        })
    }

    pub(crate) fn begin(self: &Arc<Self>) -> ActiveRequest {
        let mut state = lock(&self.state);
        state.in_flight += 1;
        state.started += 1;
        drop(state);
        self.changed.notify_one();
        ActiveRequest(self.clone())
    }

    /// Resolves once the connection has had no request in flight for its
    /// idle timeout: `header_read_timeout` before the first request,
    /// `idle_timeout` after, `overload_idle_timeout` while `pressure` is set.
    pub(crate) async fn idle_expired(&self, limits: &ConnectionLimits, mut pressure: watch::Receiver<bool>) {
        let mut follow_pressure = true;
        loop {
            // Created before reading the state: a change in between still wakes us.
            let changed = self.changed.notified();
            let pressured = follow_pressure && *pressure.borrow_and_update();
            let deadline = {
                let state = lock(&self.state);
                (state.in_flight == 0).then(|| {
                    let timeout = if pressured {
                        limits.overload_idle_timeout
                    } else if state.started == 0 {
                        limits.header_read_timeout
                    } else {
                        limits.idle_timeout
                    };
                    state.idle_since + timeout
                })
            };
            let Some(deadline) = deadline else {
                changed.await;
                continue;
            };
            if deadline <= Instant::now() {
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {}
                _ = changed => {}
                // An error means the sender is gone: nothing to follow any more.
                res = pressure.changed(), if follow_pressure => follow_pressure = res.is_ok(),
            }
        }
    }

    /// Resolves once no request has been in flight for `grace`, counted
    /// from this call at the earliest.
    pub(crate) async fn quiet_for(&self, grace: Duration) {
        let start = Instant::now();
        loop {
            let changed = self.changed.notified();
            let deadline = {
                let state = lock(&self.state);
                (state.in_flight == 0).then(|| state.idle_since.max(start) + grace)
            };
            match deadline {
                Some(deadline) if deadline <= Instant::now() => return,
                Some(deadline) => tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {}
                    _ = changed => {}
                },
                None => changed.await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_connection_limit_from_the_open_file_limit() {
        assert_eq!(auto_max_connections(None), DEFAULT_MAX_CONNECTIONS);
        assert_eq!(auto_max_connections(Some(256)), 96);
        assert_eq!(auto_max_connections(Some(1024)), 384);
        assert_eq!(auto_max_connections(Some(10)), MIN_AUTO_MAX_CONNECTIONS);
        assert_eq!(auto_max_connections(Some(1 << 20)), DEFAULT_MAX_CONNECTIONS);
        let limits = ConnectionLimits { max_connections: Some(0), ..ConnectionLimits::default() };
        assert_eq!(limits.effective_max_connections(), 1);
        let auto = ConnectionLimits::default().effective_max_connections();
        assert!((MIN_AUTO_MAX_CONNECTIONS..=DEFAULT_MAX_CONNECTIONS).contains(&auto), "{auto}");
    }

    #[test]
    fn groups_addresses() {
        let key = |s: &str| addr_key(s.parse().unwrap());
        assert_eq!(key("127.0.0.1"), None);
        assert_eq!(key("::1"), None);
        assert_eq!(key("::ffff:127.0.0.1"), None);
        assert_eq!(key("::ffff:203.0.113.7"), key("203.0.113.7"));
        assert_ne!(key("203.0.113.7"), key("203.0.113.8"));
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:2:ffff::9"));
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    }

    fn new_limiter(max: usize, per_ip: usize) -> Arc<ConnLimiter> {
        ConnLimiter::new(&ConnectionLimits {
            max_connections: Some(max),
            max_connections_per_ip: per_ip,
            ..ConnectionLimits::default()
        })
    }

    #[tokio::test]
    async fn caps_connections_per_address() {
        let limiter = new_limiter(100, 2);
        let a: IpAddr = "203.0.113.7".parse().unwrap();
        let b: IpAddr = "2001:db8::1".parse().unwrap();
        let a1 = limiter.admit(limiter.reserve().await, a).unwrap();
        let a2 = limiter.admit(limiter.reserve().await, a).unwrap();
        assert!(limiter.admit(limiter.reserve().await, a).is_none());
        // The refused connection's slot was given back.
        assert_eq!(limiter.slots.available_permits(), 98);
        // Other addresses and loopback are not affected.
        let b1 = limiter.admit(limiter.reserve().await, b).unwrap();
        let local: Vec<_> = (0..5)
            .map(|_| limiter.admit(limiter.slots.clone().try_acquire_owned().unwrap(), "127.0.0.1".parse().unwrap()))
            .collect();
        assert!(local.iter().all(Option::is_some));
        // Closing one frees a place for that address.
        drop(a1);
        assert!(limiter.admit(limiter.reserve().await, a).is_some());
        drop(local);
        // Only addresses with connections open are kept (loopback is never counted).
        assert_eq!(lock(&limiter.per_ip).len(), 2);
        drop((a2, b1));
        assert!(lock(&limiter.per_ip).is_empty());
        assert_eq!(limiter.slots.available_permits(), 100);

        let unlimited = new_limiter(10, 0);
        let slots: Vec<_> =
            (0..10).map(|_| unlimited.admit(unlimited.slots.clone().try_acquire_owned().unwrap(), a)).collect();
        assert!(slots.iter().all(Option::is_some));
    }

    #[tokio::test]
    async fn signals_pressure_at_the_limit_until_slots_free_up() {
        let limiter = new_limiter(20, 0);
        let mut pressure = limiter.pressure();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let mut open: Vec<ConnSlot> = Vec::new();
        for _ in 0..20 {
            open.push(limiter.admit(limiter.reserve().await, ip).unwrap());
        }
        assert!(!*pressure.borrow_and_update());
        let waiting = tokio::spawn({
            let limiter = limiter.clone();
            async move { limiter.reserve().await }
        });
        pressure.changed().await.unwrap();
        assert!(*pressure.borrow_and_update());
        // The first freed slot goes to the waiting accept loop: still at the limit.
        open.pop();
        let _reserved = waiting.await.unwrap();
        assert!(*pressure.borrow());
        open.pop();
        assert!(*pressure.borrow(), "released before 10% of the slots were free");
        open.pop();
        assert!(!*pressure.borrow());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_deadlines_follow_requests_and_pressure() {
        let limits = ConnectionLimits {
            header_read_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            overload_idle_timeout: Duration::from_secs(5),
            ..ConnectionLimits::default()
        };
        let (pressure_tx, _) = watch::channel(false);
        let elapsed = |activity: Arc<Activity>, rx: watch::Receiver<bool>, limits: ConnectionLimits| {
            tokio::spawn(async move {
                let started = Instant::now();
                activity.idle_expired(&limits, rx).await;
                started.elapsed()
            })
        };

        // Never any request: the first-request (header) timeout.
        let fresh = elapsed(Activity::new(), pressure_tx.subscribe(), limits.clone());
        assert_eq!(fresh.await.unwrap(), Duration::from_secs(30));

        // A request in flight holds the connection open indefinitely; then the idle timeout runs.
        let activity = Activity::new();
        let request = activity.begin();
        let watch = elapsed(activity.clone(), pressure_tx.subscribe(), limits.clone());
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert!(!watch.is_finished());
        drop(request);
        assert_eq!(watch.await.unwrap(), Duration::from_secs(3660));

        // Pressure shortens the wait of idle connections, including ones idle for a while.
        let activity = Activity::new();
        drop(activity.begin());
        let watch = elapsed(activity.clone(), pressure_tx.subscribe(), limits.clone());
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(!watch.is_finished());
        pressure_tx.send_replace(true);
        assert_eq!(watch.await.unwrap(), Duration::from_secs(10));

        // `quiet_for` counts from the call at the earliest, and waits for requests to end.
        let activity = Activity::new();
        let request = activity.begin();
        let quiet = tokio::spawn({
            let activity = activity.clone();
            async move {
                let started = Instant::now();
                activity.quiet_for(Duration::from_secs(2)).await;
                started.elapsed()
            }
        });
        tokio::time::sleep(Duration::from_secs(7)).await;
        drop(request);
        assert_eq!(quiet.await.unwrap(), Duration::from_secs(9));
    }
}
