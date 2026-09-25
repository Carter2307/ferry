//! Listeners, accept loops, TLS termination and graceful shutdown.

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
use crate::handler::{ConnInfo, Proxy};
use crate::routes::RouteTable;

/// How long in-flight requests get to finish once shutdown starts.
pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// Slow or stalled TLS handshakes are dropped after this.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time a client has to send a complete HTTP/1 request head (this also bounds
/// how long an idle keep-alive connection stays open).
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// HTTP/2 keep-alive pings detect dead client connections.
const H2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const H2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

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
    let builder = Arc::new(connection_builder());
    let ctx = AcceptCtx { proxy, builder, tasks: tasks.clone(), shutdown: shutdown.clone(), force: force.clone() };

    log_listening("http", &http);
    let http_loop = accept_loop(http, None, ctx.clone());
    let https_loop = async {
        if let Some((listener, acceptor)) = https {
            log_listening("https", &listener);
            accept_loop(listener, Some(acceptor), ctx.clone()).await;
        }
    };
    tokio::join!(http_loop, https_loop);
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

fn connection_builder() -> ConnBuilder {
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT).keep_alive(true);
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
    tasks: TaskTracker,
    shutdown: CancellationToken,
    force: CancellationToken,
}

async fn accept_loop(listener: TcpListener, tls: Option<TlsAcceptor>, ctx: AcceptCtx) {
    let mut backoff = Duration::from_millis(5);
    loop {
        let accepted = tokio::select! {
            _ = ctx.shutdown.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, remote)) => {
                backoff = Duration::from_millis(5);
                ctx.tasks.spawn(handle_connection(stream, remote, tls.clone(), ctx.clone()));
            }
            Err(e) => {
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

async fn handle_connection(stream: TcpStream, remote: SocketAddr, tls: Option<TlsAcceptor>, ctx: AcceptCtx) {
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!("proxy: set_nodelay for {remote}: {e}");
    }
    let Some(acceptor) = tls else {
        return serve_connection(TokioIo::new(stream), ConnInfo { remote, tls: false }, ctx).await;
    };
    let handshake = tokio::select! {
        _ = ctx.shutdown.cancelled() => return,
        res = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)) => res,
    };
    match handshake {
        Ok(Ok(stream)) => serve_connection(TokioIo::new(stream), ConnInfo { remote, tls: true }, ctx).await,
        Ok(Err(e)) => tracing::debug!("proxy: TLS handshake with {remote} failed: {e}"),
        Err(_) => tracing::debug!("proxy: TLS handshake with {remote} timed out"),
    }
}

async fn serve_connection<I>(io: I, info: ConnInfo, ctx: AcceptCtx)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let proxy = ctx.proxy.clone();
    let service = service_fn(move |req| proxy.clone().handle(info, req));
    let conn = ctx.builder.serve_connection_with_upgrades(io, service);
    tokio::pin!(conn);
    let result = tokio::select! {
        res = conn.as_mut() => res,
        _ = ctx.shutdown.cancelled() => {
            // Finish in-flight requests, refuse new ones, then close.
            conn.as_mut().graceful_shutdown();
            tokio::select! {
                res = conn.as_mut() => res,
                _ = ctx.force.cancelled() => {
                    tracing::debug!("proxy: dropping connection from {} after the drain timeout", info.remote);
                    return;
                }
            }
        }
    };
    if let Err(e) = result {
        tracing::debug!("proxy: connection from {} closed with error: {e}", info.remote);
    }
}
