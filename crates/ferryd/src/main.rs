//! `ferryd` — the Ferry server. Wires together the store, Docker, builder,
//! proxy, optional TLS manager, engine and API.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use ferry_core::tls::TlsHooks;
use ferry_core::{CancellationToken, Config, Engine, Store};
use tracing_subscriber::EnvFilter;

/// Ferry server: a self-hosted Render alternative.
#[derive(Debug, Parser)]
#[command(name = "ferryd", version, about)]
struct Args {
    /// Directory for the database, logs, build scratch space and certificates.
    #[arg(long, env = "FERRY_DATA_DIR", default_value = "./ferry-data")]
    data_dir: PathBuf,
    /// API + dashboard listen address.
    #[arg(long, env = "FERRY_API_ADDR", default_value = "127.0.0.1:7878")]
    api_addr: SocketAddr,
    /// Public HTTP proxy listen address.
    #[arg(long, env = "FERRY_PROXY_ADDR", default_value = "0.0.0.0:8080")]
    proxy_addr: SocketAddr,
    /// Public HTTPS proxy listen address (enables TLS together with --acme-email).
    #[arg(long, env = "FERRY_HTTPS_ADDR")]
    https_addr: Option<SocketAddr>,
    /// Services are reachable at <name>.<base-domain>.
    #[arg(long, env = "FERRY_BASE_DOMAIN", default_value = "localhost")]
    base_domain: String,
    /// Port shown in public URLs (defaults to the proxy port).
    #[arg(long, env = "FERRY_PUBLIC_PORT")]
    public_port: Option<u16>,
    /// Host that serves the dashboard through the proxy ("none" to disable).
    /// Default: ferry.<base-domain>.
    #[arg(long, env = "FERRY_DASHBOARD_HOST")]
    dashboard_host: Option<String>,
    /// Prefix for Docker resources; lets several Ferry servers share one daemon.
    #[arg(long, env = "FERRY_NAME_PREFIX", default_value = "ferry")]
    name_prefix: String,
    /// API token. Default: read from / generated into <data-dir>/api_token.
    #[arg(long, env = "FERRY_API_TOKEN", hide_env_values = true)]
    api_token: Option<String>,
    /// Secret for GitHub webhook signatures (enables POST /hooks/github).
    #[arg(long, env = "FERRY_GITHUB_WEBHOOK_SECRET", hide_env_values = true)]
    github_webhook_secret: Option<String>,
    /// Email for Let's Encrypt; enables automatic HTTPS with --https-addr.
    #[arg(long, env = "FERRY_ACME_EMAIL")]
    acme_email: Option<String>,
    /// Use the Let's Encrypt staging environment.
    #[arg(long, env = "FERRY_ACME_STAGING")]
    acme_staging: bool,
    /// Custom ACME directory URL (e.g. Pebble for testing).
    #[arg(long, env = "FERRY_ACME_DIRECTORY")]
    acme_directory: Option<String>,
    /// Maximum concurrent builds.
    #[arg(long, env = "FERRY_BUILD_CONCURRENCY", default_value_t = 2)]
    build_concurrency: usize,
    /// Container port used when nothing else specifies one.
    #[arg(long, env = "FERRY_DEFAULT_PORT", default_value_t = 10000)]
    default_port: u16,
    /// Built images kept per service (for rollbacks).
    #[arg(long, env = "FERRY_KEEP_IMAGES", default_value_t = 5)]
    keep_images: usize,
    /// docker CLI used for builds.
    #[arg(long, env = "FERRY_DOCKER_BIN", default_value = "docker")]
    docker_bin: String,
    /// Seconds a new deploy has to pass its health check.
    #[arg(long, env = "FERRY_HEALTH_TIMEOUT", default_value_t = 120)]
    health_check_timeout: u64,
    /// Host name printed in external datastore connection strings.
    #[arg(long, env = "FERRY_ADVERTISE_HOST", default_value = "127.0.0.1")]
    advertise_host: String,
}

/// Load the API token from disk, or generate and persist a new one.
fn load_or_create_token(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join("api_token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let token = format!("fy_{}", ferry_core::ids::random_secret(40));
    std::fs::write(&path, &token).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

fn build_config(args: Args) -> anyhow::Result<Config> {
    std::fs::create_dir_all(&args.data_dir)
        .with_context(|| format!("creating data dir {}", args.data_dir.display()))?;
    let data_dir = args.data_dir.canonicalize().unwrap_or(args.data_dir.clone());
    let api_token = match args.api_token {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => load_or_create_token(&data_dir)?,
    };
    let dashboard_host = match args.dashboard_host.as_deref() {
        Some("none") | Some("") => None,
        Some(h) => Some(h.to_ascii_lowercase()),
        None => Some(format!("ferry.{}", args.base_domain)),
    };
    let config = Config {
        data_dir,
        api_addr: args.api_addr,
        proxy_addr: args.proxy_addr,
        proxy_https_addr: args.https_addr,
        base_domain: args.base_domain.to_ascii_lowercase(),
        public_port: args.public_port,
        dashboard_host,
        name_prefix: args.name_prefix,
        api_token,
        github_webhook_secret: args.github_webhook_secret.filter(|s| !s.is_empty()),
        acme_email: args.acme_email.filter(|s| !s.is_empty()),
        acme_staging: args.acme_staging,
        build_concurrency: args.build_concurrency.max(1),
        default_port: args.default_port,
        keep_images: args.keep_images.max(1),
        docker_bin: args.docker_bin,
        health_check_timeout_secs: args.health_check_timeout.max(5),
        datastore_bind_ip: "127.0.0.1".to_string(),
        advertise_host: args.advertise_host,
    };
    for dir in [config.logs_dir(), config.builds_dir(), config.repos_dir(), config.uploads_dir(), config.certs_dir()] {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(config)
}

/// Address the proxy uses to reach the API (unspecified IP → loopback).
fn loopback_of(addr: SocketAddr) -> SocketAddr {
    let ip = if addr.ip().is_unspecified() { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { addr.ip() };
    SocketAddr::new(ip, addr.port())
}

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let args = Args::parse();
    let acme_directory = args.acme_directory.clone();
    let config = Arc::new(build_config(args)?);

    let store = Store::open(&config.db_path()).await.context("opening database")?;
    let docker =
        ferry_docker::Docker::connect().await.context("cannot connect to Docker — is the Docker daemon running?")?;
    let docker_version = docker.version().await.ok();
    let naming = config.naming();
    docker
        .ensure_network(&naming.network(), &naming.base_labels("network"))
        .await
        .context("creating Docker network")?;

    let builder = ferry_build::Builder::new(config.builds_dir(), config.repos_dir(), config.docker_bin.clone());
    let routes = ferry_proxy::RouteTable::new();
    if let Some(host) = &config.dashboard_host {
        routes.set_service_routes("__dashboard", &[host.clone()], vec![loopback_of(config.api_addr)]);
    }

    let shutdown = CancellationToken::new();

    let mut proxy_config = ferry_proxy::ProxyConfig::http(config.proxy_addr);
    if config.tls_enabled() {
        let manager = ferry_tls::CertManager::new(ferry_tls::TlsConfig {
            certs_dir: config.certs_dir(),
            acme_email: config.acme_email.clone().unwrap_or_default(),
            staging: config.acme_staging,
            directory_url: acme_directory,
        })
        .await
        .context("starting TLS manager")?;
        let hosts_routes = routes.clone();
        manager.spawn(Arc::new(move || hosts_routes.hosts()), shutdown.clone());
        proxy_config.https_addr = config.proxy_https_addr;
        proxy_config.tls = Some(manager.server_config());
        proxy_config.tls_hooks = Some(manager.hooks() as Arc<dyn TlsHooks>);
        proxy_config.redirect_https = true;
    }

    let engine = ferry_engine::FerryEngine::new(config.clone(), store.clone(), docker, builder, routes.clone());
    engine.start(shutdown.clone()).await.context("starting engine")?;

    let proxy_shutdown = shutdown.clone();
    let proxy_routes = routes.clone();
    let proxy_task = tokio::spawn(async move {
        if let Err(e) = ferry_proxy::serve(proxy_config, proxy_routes, proxy_shutdown.clone()).await {
            tracing::error!("proxy stopped: {e}");
            proxy_shutdown.cancel();
        }
    });

    let app = ferry_api::router(ferry_api::AppState {
        config: config.clone(),
        store,
        engine: engine.clone() as Arc<dyn Engine>,
        docker_version: docker_version.clone(),
    });
    let listener = tokio::net::TcpListener::bind(config.api_addr)
        .await
        .with_context(|| format!("binding API address {}", config.api_addr))?;

    let api_url = format!("http://{}", loopback_of(config.api_addr));
    println!();
    println!("  Ferry {} is running", ferry_core::VERSION);
    println!("  Dashboard + API : {api_url}");
    if let Some(url) = config.dashboard_url() {
        println!("  Dashboard (proxy): {url}");
    }
    println!("  Apps            : {}", config.url_for_host(&format!("<name>.{}", config.base_domain)));
    println!("  Docker          : {}", docker_version.as_deref().unwrap_or("unknown"));
    println!("  Data            : {}", config.data_dir.display());
    println!("  API token       : {}", config.api_token);
    println!();
    println!("  Log in with:  ferry login --server {api_url} --token {}", config.api_token);
    println!();

    let signal_token = shutdown.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutting down");
        signal_token.cancel();
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .await
        .context("API server")?;
    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), proxy_task).await;
    Ok(())
}
