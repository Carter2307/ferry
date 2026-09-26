//! `ferryd` — the Ferry server. Wires together the store, Docker, builder,
//! proxy, optional TLS manager, engine and API.

use std::future::IntoFuture;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
    /// Host that serves the dashboard + API through the public proxy ("none"
    /// to disable). Default: ferry.<base-domain> when the base domain is local
    /// (e.g. localhost), otherwise disabled — enable it explicitly, ideally
    /// with HTTPS, since it exposes the admin API.
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
    /// Take ownership of Docker resources (with this name prefix) that another
    /// Ferry data directory owns — e.g. after moving the data directory.
    #[arg(long)]
    take_over: bool,
}

/// Make the data dir private (it holds secrets: env vars, datastore
/// passwords, credentialed repo URLs, logs, uploads).
fn secure_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating data dir {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting permissions of {}", dir.display()))?;
    }
    Ok(())
}

/// Write a small secret file readable only by the current user.
fn write_private_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(contents.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Read a secret file, tightening its permissions if they're too open.
fn read_private_file(path: &Path) -> Option<String> {
    let value = std::fs::read_to_string(path).ok()?.trim().to_string();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    (!value.is_empty()).then_some(value)
}

/// Hold an exclusive lock on `<data-dir>/ferryd.lock` for the process lifetime.
fn lock_data_dir(data_dir: &Path) -> anyhow::Result<std::fs::File> {
    let path = data_dir.join("ferryd.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("another ferryd is already running with data directory {}", data_dir.display())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {}", path.display())),
    }
}

/// Make sure the Docker resources of this name prefix belong to this data
/// directory, so two servers can never reconcile away each other's
/// containers. Ownership is recorded as labels on a marker volume
/// `<prefix>-owner`.
async fn claim_docker_prefix(
    docker: &ferry_docker::Docker,
    store: &Store,
    config: &Config,
    take_over: bool,
) -> anyhow::Result<()> {
    const OWNER: &str = "ferry.owner";
    const DATA_DIR: &str = "ferry.data-dir";
    let id_path = config.data_dir.join("instance_id");
    let owner_id = match read_private_file(&id_path) {
        Some(id) => id,
        None => {
            let id = ferry_core::ids::random_secret(24);
            write_private_file(&id_path, &id)?;
            id
        }
    };
    let prefix = &config.name_prefix;
    let marker = format!("{prefix}-owner");
    let naming = config.naming();
    let mut labels = naming.base_labels("owner");
    labels.insert(OWNER.to_string(), owner_id.clone());
    labels.insert(DATA_DIR.to_string(), config.data_dir.display().to_string());

    let existing = match docker.bollard().inspect_volume(&marker).await {
        Ok(v) => Some(v.labels),
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => None,
        Err(e) => return Err(anyhow::anyhow!("inspecting Docker volume {marker}: {e}")),
    };
    let conflict_hint = "Run this server with a different --name-prefix, stop the other server, or pass \
         --take-over to adopt the resources (the other server's containers will then be managed — and possibly \
         removed — by this one).";
    match existing {
        Some(l) if l.get(OWNER) == Some(&owner_id) => return Ok(()),
        Some(l) if !take_over => anyhow::bail!(
            "Docker resources with prefix '{prefix}' belong to another Ferry server (data dir {}). {conflict_hint}",
            l.get(DATA_DIR).map(String::as_str).unwrap_or("unknown")
        ),
        Some(_) => {
            tracing::warn!(prefix = %prefix, "taking over Docker resources owned by another Ferry data dir");
            docker.remove_volume(&marker).await.context("removing the old owner marker")?;
        }
        None => {
            // No marker: refuse to adopt containers of an unknown server when
            // this data dir is brand new (typical mistake: running ferryd from
            // another directory with the default ./ferry-data).
            let others = docker
                .list_containers(&[(ferry_core::naming::LABEL_INSTANCE, prefix.as_str())], true)
                .await
                .context("listing Docker containers")?;
            let fresh = store.list_services().await?.is_empty() && store.list_datastores().await?.is_empty();
            if !others.is_empty() && fresh && !take_over {
                anyhow::bail!(
                    "found {} Docker container(s) with prefix '{prefix}' that this (empty) data dir {} doesn't know. \
                     They probably belong to another Ferry server. {conflict_hint}",
                    others.len(),
                    config.data_dir.display()
                );
            }
        }
    }
    docker.ensure_volume(&marker, &labels).await.context("creating the owner marker volume")?;
    Ok(())
}

/// Load the API token from disk, or generate and persist a new one (0600).
fn load_or_create_token(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join("api_token");
    if let Some(t) = read_private_file(&path) {
        return Ok(t);
    }
    let _ = std::fs::remove_file(&path); // empty file
    let token = format!("fy_{}", ferry_core::ids::random_secret(40));
    write_private_file(&path, &token)?;
    Ok(token)
}

fn build_config(args: Args) -> anyhow::Result<Config> {
    secure_dir(&args.data_dir)?;
    let data_dir = args.data_dir.canonicalize().unwrap_or(args.data_dir.clone());
    let api_token = match args.api_token {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => load_or_create_token(&data_dir)?,
    };
    let dashboard_host = match args.dashboard_host.as_deref() {
        Some("none") | Some("") => None,
        Some(h) => Some(h.to_ascii_lowercase()),
        None => {
            // Only on by default for local development domains: on a public
            // domain it would expose the admin API to the internet.
            let host = format!("ferry.{}", args.base_domain.to_ascii_lowercase());
            ferry_core::config::is_local_host(&host).then_some(host)
        }
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    // The proxy sizes its connection cap from the open-file limit; raise the
    // soft limit to the hard one (launchd's default soft limit is only 256).
    match rlimit::increase_nofile_limit(u64::MAX) {
        Ok(n) => tracing::debug!(nofile = n, "open file limit"),
        Err(e) => tracing::warn!("could not raise the open file limit: {e}"),
    }

    let args = Args::parse();
    let acme_directory = args.acme_directory.clone();
    let take_over = args.take_over;
    let config = Arc::new(build_config(args)?);
    // Held for the whole process lifetime.
    let _data_dir_lock = lock_data_dir(&config.data_dir)?;

    // Fail fast (before touching Docker) if a public listener can't bind.
    for (what, addr) in
        [("proxy", Some(config.proxy_addr)), ("HTTPS proxy", config.proxy_https_addr), ("API", Some(config.api_addr))]
    {
        if let Some(addr) = addr.filter(|_| what != "HTTPS proxy" || config.tls_enabled()) {
            std::net::TcpListener::bind(addr).with_context(|| format!("cannot listen on {addr} ({what} address)"))?;
        }
    }

    let store = Store::open(&config.db_path()).await.context("opening database")?;
    let docker =
        ferry_docker::Docker::connect().await.context("cannot connect to Docker — is the Docker daemon running?")?;
    let docker_version = docker.version().await.ok();
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
        docker_version: docker_version.clone(),
        shutdown: shutdown.clone(),
    });
    let listener = tokio::net::TcpListener::bind(config.api_addr)
        .await
        .with_context(|| format!("binding API address {}", config.api_addr))?;

    let api_url = format!("http://{}", loopback_of(config.api_addr));
    println!();
    println!("  Ferry {} is running", ferry_core::VERSION);
    println!("  Dashboard + API : {api_url}");
    match config.dashboard_url() {
        Some(url) => println!("  Dashboard (proxy): {url}"),
        None => println!("  Dashboard (proxy): disabled (enable with --dashboard-host ferry.{})", config.base_domain),
    }
    println!("  Apps            : {}", config.url_for_host(&format!("<name>.{}", config.base_domain)));
    println!("  Docker          : {}", docker_version.as_deref().unwrap_or("unknown"));
    println!("  Data            : {}", config.data_dir.display());
    println!("  API token       : {}", config.api_token);
    println!();
    println!("  Log in with:  ferry login --server {api_url} --token {}", config.api_token);
    println!();

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
