//! `ferryd run`: the server itself. Wires together the store, Docker,
//! builder, proxy, optional TLS manager, engine and API, and serves until it
//! is told to stop.

use std::future::IntoFuture;
use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use ferry_core::tls::{Certificates, TlsHooks};
use ferry_core::{CancellationToken, Engine, Store};
use tracing_subscriber::EnvFilter;

use crate::banner;
use crate::cli::ServerArgs;
use crate::config::build_config;
use crate::daemon;
use crate::oom::set_oom_score_adj;
use crate::ownership::{claim_docker_prefix, lock_data_dir};

/// Address the proxy uses to reach the API (unspecified IP → loopback).
fn loopback_of(addr: SocketAddr) -> SocketAddr {
    let ip = if addr.ip().is_unspecified() { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { addr.ip() };
    SocketAddr::new(ip, addr.port())
}

/// Resolves on SIGINT (Ctrl-C) or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// `ferryd run`: the server itself, until it is told to stop.
pub fn run(args: ServerArgs, detached: bool) -> anyhow::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().context("starting the runtime")?;
    runtime.block_on(serve(args, detached))?;
    Ok(ExitCode::SUCCESS)
}

async fn serve(args: ServerArgs, detached: bool) -> anyhow::Result<()> {
    // Colors only for a person: not in `ferryd.log`, a pipe or the journal.
    let colors = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .with_ansi(colors)
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    // The proxy sizes its connection cap from the open-file limit; raise the
    // soft limit to the hard one (launchd's default soft limit is only 256).
    match rlimit::increase_nofile_limit(u64::MAX) {
        Ok(n) => tracing::debug!(nofile = n, "open file limit"),
        Err(e) => tracing::warn!("could not raise the open file limit: {e}"),
    }

    set_oom_score_adj(args.oom_score_adj);
    let acme_directory = args.acme_directory.clone();
    let take_over = args.take_over;
    let config = Arc::new(build_config(args)?);
    // Held for the whole process lifetime.
    let _data_dir_lock = lock_data_dir(&config.data_dir)?;
    // What `ferryd status` / `stop` read; removed when the server stops
    // (declared after the lock, so before the lock is released).
    let mut state = daemon::StateFile::create(&config.data_dir, detached)?;

    // Fail fast (before touching Docker) if a public listener can't bind.
    for (what, addr) in
        [("proxy", Some(config.proxy_addr)), ("HTTPS proxy", config.proxy_https_addr), ("API", Some(config.api_addr))]
    {
        if let Some(addr) = addr.filter(|_| what != "HTTPS proxy" || config.tls_enabled()) {
            std::net::TcpListener::bind(addr).with_context(|| format!("cannot listen on {addr} ({what} address)"))?;
        }
    }

    let store = Store::open(&config.db_path()).await.context("opening database")?;
    // A server without an account gets the code that creating it takes
    // (the banner prints it in a link).
    ferry_api::setup::ensure_code(&config, &store).await.context("preparing the first-run setup")?;
    // The domains services are served under: the base domain of the flag
    // and the ones connected since (DESIGN.md §21).
    ferry_core::domains::init(&store, &config).await.context("reading the domains")?;
    let docker =
        ferry_docker::Docker::connect().await.context("cannot connect to Docker — is the Docker daemon running?")?;
    let docker_version = docker.version().await.ok();
    let docker_host = match docker.host_info().await {
        Ok(host) => {
            for warning in banner::capacity_warnings(&config, &host) {
                tracing::warn!("{warning}");
            }
            Some(host)
        }
        Err(e) => {
            tracing::debug!("could not read the Docker host's capacity: {e}");
            None
        }
    };
    claim_docker_prefix(&docker, &store, &config, take_over).await?;
    let naming = config.naming();
    docker
        .ensure_network(&naming.network(), &naming.base_labels("network"))
        .await
        .context("creating Docker network")?;

    let builder = ferry_build::Builder::new(config.builds_dir(), config.repos_dir(), config.docker_bin.clone());
    let routes = ferry_proxy::RouteTable::new();
    if let Some(host) = &config.dashboard_host {
        routes.set_service_routes("__dashboard", std::slice::from_ref(host), vec![loopback_of(config.api_addr)]);
    }

    let shutdown = CancellationToken::new();

    let mut proxy_config = ferry_proxy::ProxyConfig::http(config.proxy_addr);
    // How the engine verifies that a domain reaches this server.
    proxy_config.domain_probe_id = Some(config.domains.probe_id().to_string());
    let mut certificates: Option<Arc<dyn Certificates>> = None;
    if config.tls_enabled() {
        let manager = ferry_tls::CertManager::new(ferry_tls::TlsConfig {
            certs_dir: config.certs_dir(),
            acme_email: config.acme_email.clone().unwrap_or_default(),
            staging: config.acme_staging,
            directory_url: acme_directory,
        })
        .await
        .context("starting TLS manager")?;
        // A host that starts being routed (a new service, a verified
        // domain) gets its certificate at once, not at the next minute.
        let hosts_routes = routes.clone();
        manager.spawn_with_wake(Arc::new(move || hosts_routes.hosts()), routes.hosts_changed(), shutdown.clone());
        certificates = Some(manager.certificates());
        proxy_config.https_addr = config.proxy_https_addr;
        proxy_config.tls = Some(manager.server_config());
        proxy_config.tls_hooks = Some(manager.hooks() as Arc<dyn TlsHooks>);
        proxy_config.redirect_https = true;
    }

    let engine = ferry_engine::FerryEngine::new(config.clone(), store.clone(), docker, builder, routes.clone());
    engine.start(shutdown.clone()).await.context("starting engine")?;

    let proxy_shutdown = shutdown.clone();
    let proxy_routes = routes.clone();
    let mut proxy_task = tokio::spawn(async move {
        let res = ferry_proxy::serve(proxy_config, proxy_routes, proxy_shutdown.clone()).await;
        if let Err(e) = &res {
            tracing::error!("proxy stopped: {e}");
            proxy_shutdown.cancel();
        }
        res
    });
    // Surface immediate proxy failures (e.g. a bind race) before announcing success.
    if let Ok(joined) = tokio::time::timeout(Duration::from_millis(300), &mut proxy_task).await {
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(25), engine.stopped()).await;
        return match joined {
            Ok(Ok(())) => Err(anyhow::anyhow!("proxy exited unexpectedly")),
            Ok(Err(e)) => Err(e).context("starting proxy"),
            Err(e) => Err(anyhow::anyhow!("proxy task failed: {e}")),
        };
    }

    let app = ferry_api::router(ferry_api::AppState {
        config: config.clone(),
        store,
        engine: engine.clone() as Arc<dyn Engine>,
        certificates,
        docker_version: docker_version.clone(),
        docker_cpus: docker_host.as_ref().and_then(|h| h.cpus),
        docker_memory_bytes: docker_host.as_ref().and_then(|h| h.memory_bytes),
        shutdown: shutdown.clone(),
    });
    let listener = tokio::net::TcpListener::bind(config.api_addr)
        .await
        .with_context(|| format!("binding API address {}", config.api_addr))?;

    let api_url = format!("http://{}", loopback_of(config.api_addr));
    let summary = banner::summary(&config, &api_url, docker_version.as_deref(), docker_host.as_ref());
    let ready = daemon::Ready { api_url, summary };
    println!();
    println!("  Ferry {} is running", ferry_core::VERSION);
    daemon::print_summary(&config.data_dir, &ready);
    state.set_ready(ready)?;

    // First signal: graceful shutdown. Second signal: exit immediately.
    let signal_token = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutting down (press Ctrl-C again to force)");
        signal_token.cancel();
        shutdown_signal().await;
        tracing::warn!("forced exit");
        std::process::exit(130);
    });

    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown.clone().cancelled_owned());
    let drain_deadline = {
        let token = shutdown.clone();
        async move {
            token.cancelled().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    };
    tokio::select! {
        res = server.into_future() => res.context("API server")?,
        _ = drain_deadline => tracing::warn!("API connections still open after 10s; closing them"),
    }
    shutdown.cancel();

    let proxy_result = tokio::time::timeout(Duration::from_secs(15), proxy_task).await;
    // Let the engine finish its shutdown work (stop jobs, record interrupted deploys).
    if tokio::time::timeout(Duration::from_secs(25), engine.stopped()).await.is_err() {
        tracing::warn!("engine did not stop within 25s");
    }
    match proxy_result {
        Ok(Ok(Err(e))) => Err(e).context("proxy failed"),
        Ok(Err(e)) => Err(anyhow::anyhow!("proxy task failed: {e}")),
        _ => Ok(()),
    }
}
