//! Listeners, accept loops, connection limits, TLS termination, idle
//! connection closing and graceful shutdown.

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ferry_core::{CancellationToken, Error, Result};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_util::task::TaskTracker;

use crate::ProxyConfig;
use crate::body;
use crate::handler::{ConnInfo, Proxy};
use crate::limits::{Activity, ConnLimiter, ConnSlot, ConnectionLimits};
use crate::routes::RouteTable;

/// How long in-flight requests get to finish once shutdown starts.
pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP/2 keep-alive pings detect dead client connections.
const H2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const H2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// Once a connection is being closed (idle, or shutdown), it is dropped if
/// it has still not closed after this long without a request in flight — an
/// HTTP/2 client that never acknowledges the GOAWAY, a half-sent HTTP/1 head.
pub(crate) const CLOSE_GRACE: Duration = Duration::from_secs(2);

type ConnBuilder = auto::Builder<TokioExecutor>;

/// Bind `addr`, adding context to the error.
pub(crate) async fn bind(addr: SocketAddr, what: &str) -> Result<TcpListener> {
    TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Io(io::Error::new(e.kind(), format!("binding proxy {what} listener on {addr}: {e}"))))
}

/// Serve on already-bound listeners until `shutdown` is cancelled, then stop
/// accepting and give in-flight connections up to `drain` to finish.
///
/// `https` requires `config.tls`; the `*_addr` fields of `config` are ignored
/// (the listeners' own addresses are used).
pub(crate) async fn serve_on(
    config: &ProxyConfig,
    http: TcpListener,
    https: Option<TcpListener>,
    routes: RouteTable,
    shutdown: CancellationToken,
    drain: Duration,
) -> Result<()> {
    let https = match (https, &config.tls) {
        (Some(listener), Some(tls)) => Some((listener, TlsAcceptor::from(tls.clone()))),
        (Some(listener), None) => {
            let addr = listener.local_addr().map_or_else(|_| "?".to_string(), |a| a.to_string());
            return Err(Error::invalid(format!("proxy HTTPS listener {addr} requires a TLS configuration")));
        }
        (None, _) => None,
    };
    let https_port = match &https {
        Some((listener, _)) => Some(listener.local_addr()?.port()),
        None => None,
    };

    let tasks = TaskTracker::new();
    // Cancelled when the drain deadline passes: connections still open are dropped.
    let force = CancellationToken::new();
    let proxy = Arc::new(Proxy::new(
        routes,
        config.tls_hooks.clone(),
        config.redirect_https,
        https_port,
        tasks.clone(),
        shutdown.clone(),
    ));
    let limits = Arc::new(config.limits.clone());
    let limiter = ConnLimiter::new(&limits);
    let builder = Arc::new(connection_builder(&limits));
    let ctx = AcceptCtx {
        proxy,
        builder,
        limits,
        limiter: limiter.clone(),
        tasks: tasks.clone(),
        shutdown: shutdown.clone(),
        force: force.clone(),
    };

    log_listening("http", &http);
    if let Some((listener, _)) = &https {
        log_listening("https", listener);
    }
    let per_ip = match limiter.max_connections_per_ip() {
        0 => "no per-address limit".to_string(),
        n => format!("{n} per client address"),
    };
    tracing::info!("proxy: accepting up to {} connections ({per_ip})", limiter.max_connections());
    accept_loop(http, https, ctx).await;
    // Both listeners are dropped at this point: new connections are refused.

    tasks.close();
    if !tasks.is_empty() {
        tracing::info!("proxy: waiting up to {}s for {} open connection(s)", drain.as_secs_f32(), tasks.len());
    }
    if tokio::time::timeout(drain, tasks.wait()).await.is_err() {
        tracing::warn!("proxy: closing {} connection(s) still open after {}s", tasks.len(), drain.as_secs_f32());
        force.cancel();
        // Every task observes `force` promptly; this only bounds a pathological case.
        let _ = tokio::time::timeout(Duration::from_secs(2), tasks.wait()).await;
    }
    tracing::info!("proxy stopped");
    Ok(())
}

fn log_listening(scheme: &str, listener: &TcpListener) {
    match listener.local_addr() {
        Ok(addr) => tracing::info!("proxy listening on {scheme}://{addr}"),
        Err(e) => tracing::info!("proxy listening ({scheme}; address unavailable: {e})"),
    }
}

fn connection_builder(limits: &ConnectionLimits) -> ConnBuilder {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    // Bounds each HTTP/1 request head, including the wait for the next one on
    // an idle keep-alive connection. The time before the protocol is known
    // (and HTTP/2 idleness) is covered by `Activity::idle_expired`.
    builder.http1().timer(TokioTimer::new()).header_read_timeout(limits.header_read_timeout).keep_alive(true);
    builder
        .http2()
        .timer(TokioTimer::new())
        .keep_alive_interval(H2_KEEP_ALIVE_INTERVAL)
        .keep_alive_timeout(H2_KEEP_ALIVE_TIMEOUT);
    builder
}

#[derive(Clone)]
struct AcceptCtx {
    proxy: Arc<Proxy>,
    builder: Arc<ConnBuilder>,
    limits: Arc<ConnectionLimits>,
    limiter: Arc<ConnLimiter>,
    tasks: TaskTracker,
    shutdown: CancellationToken,
    force: CancellationToken,
}

/// Accept on both listeners (one loop, so that the slot it holds while
/// waiting serves whichever listener gets the next client).
async fn accept_loop(http: TcpListener, https: Option<(TcpListener, TlsAcceptor)>, ctx: AcceptCtx) {
    let mut backoff = Duration::from_millis(5);
    loop {
        // Take a connection slot first: at the limit, clients wait in the
        // listen backlog instead of costing a descriptor each.
        let slot = tokio::select! {
            _ = ctx.shutdown.cancelled() => return,
            slot = ctx.limiter.reserve() => slot,
        };
        let (accepted, tls) = tokio::select! {
            _ = ctx.shutdown.cancelled() => return,
            accepted = http.accept() => (accepted, None),
            accepted = accept_tls(https.as_ref()) => accepted,
        };
        match accepted {
            Ok((stream, remote)) => {
                backoff = Duration::from_millis(5);
                match ctx.limiter.admit(slot, remote.ip()) {
                    Some(slot) => {
                        ctx.tasks.spawn(handle_connection(stream, remote, tls, slot, ctx.clone()));
                    }
                    // Too many connections from that address: close it now.
                    None => drop(stream),
                }
            }
            Err(e) => {
                drop(slot);
                // Per-connection failures (ECONNABORTED…) and resource
                // exhaustion (EMFILE…) are not fatal; back off and retry.
                tracing::warn!("proxy: accept failed: {e}");
                tokio::select! {
                    _ = ctx.shutdown.cancelled() => return,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(1));
            }
        }
    }
}

/// Next client of the HTTPS listener, with the acceptor for its handshake
/// (never resolves without an HTTPS listener).
async fn accept_tls(
    https: Option<&(TcpListener, TlsAcceptor)>,
) -> (io::Result<(TcpStream, SocketAddr)>, Option<TlsAcceptor>) {
    match https {
        Some((listener, acceptor)) => (listener.accept().await, Some(acceptor.clone())),
        None => std::future::pending().await,
    }
}

async fn handle_connection(
    stream: TcpStream,
    remote: SocketAddr,
    tls: Option<TlsAcceptor>,
    slot: ConnSlot,
    ctx: AcceptCtx,
) {
    tune_client_socket(&stream, remote);
    let slot = Arc::new(slot);
    let Some(acceptor) = tls else {
        let info = ConnInfo { remote, tls: false, slot };
        return serve_connection(TokioIo::new(stream), info, ctx).await;
    };
    let handshake = tokio::select! {
        _ = ctx.shutdown.cancelled() => return,
        res = tokio::time::timeout(ctx.limits.tls_handshake_timeout, acceptor.accept(stream)) => res,
    };
    match handshake {
        Ok(Ok(stream)) => serve_connection(TokioIo::new(stream), ConnInfo { remote, tls: true, slot }, ctx).await,
        Ok(Err(e)) => tracing::debug!("proxy: TLS handshake with {remote} failed: {e}"),
        Err(_) => tracing::debug!("proxy: TLS handshake with {remote} timed out"),
    }
}

/// `TCP_NODELAY` (hyper already coalesces its writes) and TCP keep-alive
/// probing on an accepted client socket.
fn tune_client_socket(stream: &TcpStream, remote: SocketAddr) {
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!("proxy: set_nodelay for {remote}: {e}");
    }
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&tcp_keepalive()) {
        tracing::debug!("proxy: enabling TCP keep-alive for {remote}: {e}");
    }
}

/// Keep-alive probing of client sockets: the first probe after a minute
/// without traffic, then every 15 s; after 4 unanswered probes the kernel
/// resets the socket. This detects peers that vanished (machine gone, NAT
/// entry expired) on the connections the timeouts deliberately leave open —
/// quiet websocket tunnels, long polls, SSE streams between events — so they
/// don't hold a connection slot forever. Live peers answer the probes in
/// their kernel, unnoticed by either application.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "windows"
))]
fn tcp_keepalive() -> socket2::TcpKeepalive {
    socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(15))
        .with_retries(4)
}

/// Keep-alive probing with the system's timings.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "windows"
)))]
fn tcp_keepalive() -> socket2::TcpKeepalive {
    socket2::TcpKeepalive::new()
}

async fn serve_connection<I>(io: I, info: ConnInfo, ctx: AcceptCtx)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let remote = info.remote;
    let activity = Activity::new();
    let service = service_fn({
        let proxy = ctx.proxy.clone();
        let activity = activity.clone();
        move |req| {
            // Counts as in flight until the response body is sent or dropped.
            let active = activity.begin();
            let handled = proxy.clone().handle(info.clone(), req);
            async move {
                let resp = match handled.await {
                    Ok(resp) => resp,
                    Err(never) => match never {},
                };
                Ok::<_, Infallible>(resp.map(|b| body::with_guard(b, active)))
            }
        }
    });
    let conn = ctx.builder.serve_connection_with_upgrades(io, service);
    tokio::pin!(conn);

    let closing = tokio::select! {
        res = conn.as_mut() => return log_closed(remote, res),
        _ = ctx.shutdown.cancelled() => "shutdown",
        // Nothing sent (or not a full request head) since the connection
        // opened, or no request for a while: a slot and a descriptor for nothing.
        _ = activity.idle_expired(&ctx.limits, ctx.limiter.pressure()) => "idle",
    };
    tracing::debug!("proxy: closing connection from {remote} ({closing})");
    // Finish in-flight requests, refuse new ones, then close.
    conn.as_mut().graceful_shutdown();
    tokio::select! {
        res = conn.as_mut() => log_closed(remote, res),
        _ = activity.quiet_for(CLOSE_GRACE) => {
            tracing::debug!("proxy: dropping connection from {remote}: it did not close");
        }
        _ = ctx.force.cancelled() => {
            tracing::debug!("proxy: dropping connection from {remote} after the drain timeout");
        }
    }
}

fn log_closed(remote: SocketAddr, result: std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>) {
    if let Err(e) = result {
        tracing::debug!("proxy: connection from {remote} closed with error: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn client_sockets_get_nodelay_and_tcp_keepalive() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (accepted, remote) = listener.accept().await.unwrap();
        tune_client_socket(&accepted, remote);
        assert!(accepted.nodelay().unwrap());
        let sock = socket2::SockRef::from(&accepted);
        assert!(sock.keepalive().unwrap());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            assert_eq!(sock.tcp_keepalive_time().unwrap(), Duration::from_secs(60));
            assert_eq!(sock.tcp_keepalive_interval().unwrap(), Duration::from_secs(15));
            assert_eq!(sock.tcp_keepalive_retries().unwrap(), 4);
        }
    }
}
