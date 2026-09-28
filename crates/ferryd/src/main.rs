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
use ferry_core::{CancellationToken, Config, Engine, Store, resources};
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
    /// Finished job runs (and their logs) kept per service.
    #[arg(long, env = "FERRY_KEEP_JOB_RUNS", default_value_t = 100)]
    keep_job_runs: usize,
    /// docker CLI used for builds.
    #[arg(long, env = "FERRY_DOCKER_BIN", default_value = "docker")]
    docker_bin: String,
    /// Seconds a new deploy has to pass its health check.
    #[arg(long, env = "FERRY_HEALTH_TIMEOUT", default_value_t = 120)]
    health_check_timeout: u64,
    /// Host name printed in external datastore connection strings.
    #[arg(long, env = "FERRY_ADVERTISE_HOST", default_value = "127.0.0.1")]
    advertise_host: String,
    /// Memory limit of each service, job and datastore container that sets
    /// none of its own (e.g. 512M, 1G; 0 = unlimited).
    #[arg(long, env = "FERRY_DEFAULT_MEMORY_LIMIT", value_name = "SIZE")]
    #[arg(default_value = "512M", value_parser = parse_default_memory)]
    default_memory_limit: u32,
    /// CPU limit of each service, job and datastore container that sets none
    /// of its own (e.g. 0.5, 2, 500m; 0 = unlimited).
    #[arg(long, env = "FERRY_DEFAULT_CPU_LIMIT", value_name = "CPUS")]
    #[arg(default_value = "1", value_parser = parse_default_cpus)]
    default_cpu_limit: f64,
    /// Max processes + threads per container, a fork-bomb guard (0 = unlimited).
    #[arg(long, env = "FERRY_PIDS_LIMIT", value_name = "N", default_value_t = 1024)]
    pids_limit: u32,
    /// Rotate each container's Docker log at this size, with the json-file
    /// driver (e.g. 10M; 0 = keep the Docker daemon's log configuration). A
    /// daemon whose default log driver isn't json-file keeps its driver.
    #[arg(long, env = "FERRY_LOG_MAX_SIZE", value_name = "SIZE", default_value = "10M")]
    #[arg(value_parser = parse_size)]
    log_max_size: u32,
    /// Log files kept per container when rotating (current one included).
    #[arg(long, env = "FERRY_LOG_MAX_FILES", value_name = "N", default_value_t = 3)]
    #[arg(value_parser = clap::value_parser!(u32).range(1..))]
    log_max_files: u32,
    /// Deploys and new datastores fail early when less disk than this is free
    /// on the data directory's (or the local Docker root's) filesystem (e.g.
    /// 1G; 0 = no check). Recreating an existing datastore's container isn't
    /// blocked.
    #[arg(long, env = "FERRY_MIN_FREE_DISK", value_name = "SIZE", default_value = "1G")]
    #[arg(value_parser = parse_size)]
    min_free_disk: u32,
    /// Linux: OOM-killer score adjustment of ferryd itself, -1000..=1000
    /// (negative = killed after the containers when the host runs out of
    /// memory; 0 = leave unchanged). Lowering it needs root or
    /// CAP_SYS_RESOURCE (or OOMScoreAdjust= in a systemd unit).
    #[arg(long, env = "FERRY_OOM_SCORE_ADJ", value_name = "N", default_value_t = -500)]
    #[arg(allow_negative_numbers = true, value_parser = clap::value_parser!(i32).range(-1000..=1000))]
    oom_score_adj: i32,
    /// Take ownership of Docker resources (with this name prefix) that another
    /// Ferry data directory owns — e.g. after restoring a backup into a new data
    /// directory or losing the old one (moving a data directory keeps ownership).
    #[arg(long)]
    take_over: bool,
    /// Print the OpenAPI document of the API and exit (used to generate the docs site).
    #[arg(long, hide = true)]
    dump_openapi: bool,
    /// Print this command-line reference as Markdown and exit (used to generate the docs site).
    #[arg(long, hide = true)]
    dump_markdown_help: bool,
}

/// `--log-max-size` / `--min-free-disk`: a size in MiB (`10M`, `1G`, `0`).
fn parse_size(s: &str) -> Result<u32, String> {
    resources::parse_memory_mb(s).map_err(|e| e.to_string())
}

/// `--default-memory-limit`: a size in MiB within the accepted limit range,
/// or 0 (unlimited).
fn parse_default_memory(s: &str) -> Result<u32, String> {
    let mb = parse_size(s)?;
    if mb > 0 {
        resources::validate_memory_mb(mb).map_err(|e| e.to_string())?;
    }
    Ok(mb)
}

/// `--default-cpu-limit`: CPUs within the accepted limit range, or 0
/// (unlimited).
fn parse_default_cpus(s: &str) -> Result<f64, String> {
    // (A non-zero amount that rounds to 0, like `0.004`, is refused by
    // parse_cpus: it must not silently mean "unlimited".)
    let cpus = resources::parse_cpus(s).map_err(|e| e.to_string())?;
    if cpus > 0.0 {
        resources::validate_cpus(cpus).map_err(|e| e.to_string())?;
    }
    Ok(cpus)
}

/// One line describing the container limits, for the startup banner.
/// `host`: a daemon whose default log driver isn't `json-file` keeps it
/// (the engine only sets `json-file` rotation over `json-file`).
fn limits_summary(config: &Config, host: Option<&ferry_docker::HostInfo>) -> String {
    let memory = match config.default_memory_limit_mb {
        0 => "unlimited memory".to_string(),
        mb => resources::format_memory_mb(mb),
    };
    let cpu = if config.default_cpu_limit > 0.0 {
        resources::format_cpus(config.default_cpu_limit)
    } else {
        "unlimited CPU".to_string()
    };
    let pids = match config.pids_limit {
        0 => "no pids limit".to_string(),
        n => format!("{n} pids"),
    };
    let kept_driver = host.and_then(|h| h.logging_driver.as_deref()).filter(|d| *d != "json-file");
    let logs = match (config.log_max_size_mb, kept_driver) {
        (0, _) => "Docker's log settings".to_string(),
        (_, Some(driver)) => format!("Docker's {driver} log driver (kept)"),
        (mb, None) => format!("logs rotated at {} × {}", resources::format_memory_mb(mb), config.log_max_files),
    };
    format!("{memory} / {cpu} per container (default), {pids}, {logs}")
}

/// Docker version and host capacity, for the startup banner.
fn docker_summary(version: Option<&str>, host: Option<&ferry_docker::HostInfo>) -> String {
    let version = version.unwrap_or("unknown");
    let cpus = host.and_then(|h| h.cpus).map(|n| format!("{n} CPU{}", if n == 1 { "" } else { "s" }));
    let memory = host.and_then(|h| h.memory_bytes).map(|b| format!("{:.1} GiB", b as f64 / f64::from(1u32 << 30)));
    match (cpus, memory) {
        (Some(c), Some(m)) => format!("{version} ({c}, {m})"),
        (Some(x), None) | (None, Some(x)) => format!("{version} ({x})"),
        (None, None) => version.to_string(),
    }
}

/// Warnings about limits the Docker host can't honour.
fn capacity_warnings(config: &Config, host: &ferry_docker::HostInfo) -> Vec<String> {
    let mut warnings = Vec::new();
    if host.cpu_cfs_quota == Some(false) {
        warnings.push(
            "the Docker host's kernel has no CPU CFS quota support (Docker refuses CPU limits there): CPU limits are \
             not enforced on this host"
                .to_string(),
        );
    }
    if let Some(cpus) = host.cpus
        && config.default_cpu_limit > f64::from(cpus)
    {
        warnings.push(format!(
            "--default-cpu-limit {} is more than the Docker host's {cpus} CPUs; containers get at most {cpus}",
            config.default_cpu_limit
        ));
    }
    if let Some(bytes) = host.memory_bytes
        && u64::from(config.default_memory_limit_mb) << 20 > bytes
    {
        warnings.push(format!(
            "--default-memory-limit {} is more than the Docker host's memory ({} MiB)",
            resources::format_memory_mb(config.default_memory_limit_mb),
            bytes >> 20
        ));
    }
    warnings
}

/// Should ferryd write `wanted` to its `oom_score_adj` (currently
/// `current`)? Not for 0 ("leave unchanged"), and never to *raise* a lower
/// value set by the service manager (e.g. systemd's `OOMScoreAdjust=-900`).
#[cfg(any(target_os = "linux", test))]
fn should_set_oom_score_adj(current: Option<i32>, wanted: i32) -> bool {
    wanted != 0 && current != Some(wanted) && !(wanted < 0 && current.is_some_and(|c| c < wanted))
}

/// Make the kernel's OOM killer pick containers (and anything else) before
/// ferryd when the host runs out of memory. Best effort: lowering the score
/// needs root or CAP_SYS_RESOURCE. Processes ferryd spawns (git, the docker
/// CLI) are reset to 0 by ferry-build (a runaway git must not be spared);
/// containers never inherit it (dockerd starts them).
#[cfg(target_os = "linux")]
fn set_oom_score_adj(wanted: i32) {
    const PATH: &str = "/proc/self/oom_score_adj";
    let current = std::fs::read_to_string(PATH).ok().and_then(|s| s.trim().parse::<i32>().ok());
    if !should_set_oom_score_adj(current, wanted) {
        tracing::debug!(?current, wanted, "leaving oom_score_adj unchanged");
        return;
    }
    match std::fs::write(PATH, wanted.to_string()) {
        Ok(()) => tracing::debug!(wanted, "set oom_score_adj"),
        Err(e) => tracing::warn!(
            "could not set ferryd's OOM score adjustment to {wanted} ({e}): the kernel may kill ferryd instead of \
             a runaway container. Run ferryd as root, or set OOMScoreAdjust={wanted} in its systemd unit (and pass \
             --oom-score-adj 0 to silence this warning)."
        ),
    }
}

#[cfg(not(target_os = "linux"))]
fn set_oom_score_adj(wanted: i32) {
    if wanted != 0 {
        tracing::debug!(wanted, "--oom-score-adj only applies on Linux");
    }
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
        keep_job_runs: args.keep_job_runs.max(1),
        docker_bin: args.docker_bin,
        health_check_timeout_secs: args.health_check_timeout.max(5),
        datastore_bind_ip: "127.0.0.1".to_string(),
        advertise_host: args.advertise_host,
        default_memory_limit_mb: args.default_memory_limit,
        default_cpu_limit: args.default_cpu_limit,
        pids_limit: args.pids_limit,
        log_max_size_mb: args.log_max_size,
        log_max_files: args.log_max_files.max(1),
        min_free_disk_mb: u64::from(args.min_free_disk),
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
    if args.dump_openapi {
        println!("{}", ferry_api::openapi::document_json());
        return Ok(());
    }
    if args.dump_markdown_help {
        print!("{}", clap_markdown::help_markdown::<Args>());
        return Ok(());
    }
    set_oom_score_adj(args.oom_score_adj);
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
    let docker_host = match docker.host_info().await {
        Ok(host) => {
            for warning in capacity_warnings(&config, &host) {
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
        docker_cpus: docker_host.as_ref().and_then(|h| h.cpus),
        docker_memory_bytes: docker_host.as_ref().and_then(|h| h.memory_bytes),
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
    println!("  Docker          : {}", docker_summary(docker_version.as_deref(), docker_host.as_ref()));
    println!("  Data            : {}", config.data_dir.display());
    println!("  Limits          : {}", limits_summary(&config, docker_host.as_ref()));
    match config.min_free_disk_mb {
        0 => println!("  Free disk check : off"),
        mb => println!(
            "  Free disk check : deploys need {} free",
            resources::format_memory_mb(u32::try_from(mb).unwrap_or(u32::MAX))
        ),
    }
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("ferryd").chain(extra.iter().copied()))
    }

    /// The first line of a clap error (the rest is usage help).
    fn error_line(extra: &[&str]) -> String {
        let err = parse(extra).expect_err("the flags should be rejected");
        err.to_string().lines().next().unwrap_or_default().to_string()
    }

    #[test]
    fn limit_flag_defaults() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.default_memory_limit, 512);
        assert_eq!(args.default_cpu_limit, 1.0);
        assert_eq!(args.pids_limit, 1024);
        assert_eq!(args.log_max_size, 10);
        assert_eq!(args.log_max_files, 3);
        assert_eq!(args.min_free_disk, 1024);
        assert_eq!(args.oom_score_adj, -500);
        // Same defaults as the library's Config.
        let defaults = Config::default();
        assert_eq!(args.default_memory_limit, defaults.default_memory_limit_mb);
        assert_eq!(args.default_cpu_limit, defaults.default_cpu_limit);
        assert_eq!(args.pids_limit, defaults.pids_limit);
        assert_eq!(args.log_max_size, defaults.log_max_size_mb);
        assert_eq!(args.log_max_files, defaults.log_max_files);
        assert_eq!(u64::from(args.min_free_disk), defaults.min_free_disk_mb);
    }

    #[test]
    fn limit_flags_parse_human_sizes() {
        let args = parse(&[
            "--default-memory-limit",
            "1.5G",
            "--default-cpu-limit",
            "500m",
            "--pids-limit",
            "256",
            "--log-max-size",
            "50M",
            "--log-max-files",
            "5",
            "--min-free-disk",
            "10G",
            "--oom-score-adj",
            "-900",
        ])
        .unwrap();
        assert_eq!(args.default_memory_limit, 1536);
        assert_eq!(args.default_cpu_limit, 0.5);
        assert_eq!(args.pids_limit, 256);
        assert_eq!(args.log_max_size, 50);
        assert_eq!(args.log_max_files, 5);
        assert_eq!(args.min_free_disk, 10240);
        assert_eq!(args.oom_score_adj, -900);

        // 0 turns each one off.
        let off = parse(&[
            "--default-memory-limit=0",
            "--default-cpu-limit=0",
            "--pids-limit=0",
            "--log-max-size=0",
            "--min-free-disk=0",
            "--oom-score-adj=0",
        ])
        .unwrap();
        assert_eq!(
            (off.default_memory_limit, off.default_cpu_limit, off.pids_limit, off.log_max_size, off.min_free_disk),
            (0, 0.0, 0, 0, 0)
        );
        assert_eq!(off.oom_score_adj, 0);
        assert_eq!(
            parse(&["--default-memory-limit", "2048", "--default-cpu-limit", "2"]).unwrap().default_memory_limit,
            2048
        );
        assert_eq!(parse(&["--oom-score-adj", "300"]).unwrap().oom_score_adj, 300);
    }

    #[test]
    fn limit_flags_are_validated() {
        let line = error_line(&["--default-memory-limit", "8M"]);
        assert!(line.contains("--default-memory-limit") && line.contains("between 16 MiB and 1024 GiB"), "{line}");
        let line = error_line(&["--default-memory-limit", "lots"]);
        assert!(line.contains("invalid memory size 'lots'"), "{line}");
        let line = error_line(&["--default-memory-limit", "2T"]);
        assert!(line.contains("between 16 MiB and 1024 GiB"), "{line}");
        let line = error_line(&["--default-cpu-limit", "1000"]);
        assert!(line.contains("--default-cpu-limit") && line.contains("between 0.01 and 512 CPUs"), "{line}");
        let line = error_line(&["--default-cpu-limit", "0.004"]);
        assert!(line.contains("between 0.01 and 512 CPUs (got 0.004)"), "rounding must not mean unlimited: {line}");
        let line = error_line(&["--default-cpu-limit=-1"]);
        assert!(line.contains("--default-cpu-limit") && line.contains("invalid CPU amount '-1'"), "{line}");
        let line = error_line(&["--log-max-files", "0"]);
        assert!(line.contains("--log-max-files"), "{line}");
        let line = error_line(&["--log-max-size", "10X"]);
        assert!(line.contains("invalid memory size '10X'"), "{line}");
        let line = error_line(&["--min-free-disk=-1G"]);
        assert!(line.contains("--min-free-disk") && line.contains("invalid memory size '-1G'"), "{line}");
        for bad in ["-1001", "1001", "x"] {
            let line = error_line(&["--oom-score-adj", bad]);
            assert!(line.contains("--oom-score-adj"), "{bad}: {line}");
        }
        assert!(parse(&["--pids-limit", "-1"]).is_err());
    }

    #[test]
    fn limit_flags_have_env_vars() {
        let command = Args::command();
        for (id, env) in [
            ("default_memory_limit", "FERRY_DEFAULT_MEMORY_LIMIT"),
            ("default_cpu_limit", "FERRY_DEFAULT_CPU_LIMIT"),
            ("pids_limit", "FERRY_PIDS_LIMIT"),
            ("log_max_size", "FERRY_LOG_MAX_SIZE"),
            ("log_max_files", "FERRY_LOG_MAX_FILES"),
            ("min_free_disk", "FERRY_MIN_FREE_DISK"),
            ("oom_score_adj", "FERRY_OOM_SCORE_ADJ"),
        ] {
            let arg = command.get_arguments().find(|a| a.get_id() == id).unwrap_or_else(|| panic!("no flag {id}"));
            assert_eq!(arg.get_env().and_then(|e| e.to_str()), Some(env), "{id}");
        }
        command.debug_assert();
    }

    #[test]
    fn config_gets_the_limits() {
        let dir = std::env::temp_dir().join(format!("ferryd-test-{}", ferry_core::ids::random_secret(8)));
        let data_dir = dir.to_string_lossy().into_owned();
        let args = parse(&[
            "--data-dir",
            &data_dir,
            "--api-token",
            "fy_test",
            "--default-memory-limit",
            "256M",
            "--default-cpu-limit",
            "0.25",
            "--pids-limit",
            "0",
            "--log-max-size",
            "20M",
            "--log-max-files",
            "4",
            "--min-free-disk",
            "2G",
        ])
        .unwrap();
        let config = build_config(args);
        let _ = std::fs::remove_dir_all(&dir);
        let config = config.unwrap();
        assert_eq!(config.default_memory_limit_mb, 256);
        assert_eq!(config.default_cpu_limit, 0.25);
        assert_eq!(config.pids_limit, 0);
        assert_eq!(config.log_max_size_mb, 20);
        assert_eq!(config.log_max_files, 4);
        assert_eq!(config.min_free_disk_mb, 2048);
        assert_eq!(config.limits(None, None), resources::Limits { memory_mb: Some(256), cpus: Some(0.25) });
    }

    #[test]
    fn banner_lines() {
        let config = Config::default();
        assert_eq!(
            limits_summary(&config, None),
            "512 MiB / 1 CPU per container (default), 1024 pids, logs rotated at 10 MiB × 3"
        );
        let custom = Config {
            default_memory_limit_mb: 2048,
            default_cpu_limit: 1.5,
            log_max_size_mb: 100,
            log_max_files: 2,
            ..Config::default()
        };
        assert_eq!(
            limits_summary(&custom, None),
            "2 GiB / 1.5 CPUs per container (default), 1024 pids, logs rotated at 100 MiB × 2"
        );
        let off = Config {
            default_memory_limit_mb: 0,
            default_cpu_limit: 0.0,
            pids_limit: 0,
            log_max_size_mb: 0,
            ..Config::default()
        };
        assert_eq!(
            limits_summary(&off, None),
            "unlimited memory / unlimited CPU per container (default), no pids limit, Docker's log settings"
        );
        // Another log driver than json-file is kept.
        let journald = ferry_docker::HostInfo { logging_driver: Some("journald".into()), ..Default::default() };
        assert_eq!(
            limits_summary(&config, Some(&journald)),
            "512 MiB / 1 CPU per container (default), 1024 pids, Docker's journald log driver (kept)"
        );
        let json_file = ferry_docker::HostInfo { logging_driver: Some("json-file".into()), ..Default::default() };
        assert!(limits_summary(&config, Some(&json_file)).ends_with("logs rotated at 10 MiB × 3"));

        let host = ferry_docker::HostInfo { cpus: Some(10), memory_bytes: Some(8_217_317_376), ..Default::default() };
        assert_eq!(docker_summary(Some("29.2.0"), Some(&host)), "29.2.0 (10 CPUs, 7.7 GiB)");
        let one = ferry_docker::HostInfo { cpus: Some(1), ..Default::default() };
        assert_eq!(docker_summary(Some("29.2.0"), Some(&one)), "29.2.0 (1 CPU)");
        assert_eq!(docker_summary(None, None), "unknown");
    }

    #[test]
    fn capacity_warnings_for_oversized_defaults() {
        let host = ferry_docker::HostInfo { cpus: Some(2), memory_bytes: Some(1 << 30), ..Default::default() };
        assert!(capacity_warnings(&Config::default(), &host).is_empty());
        let big = Config { default_cpu_limit: 4.0, default_memory_limit_mb: 2048, ..Config::default() };
        assert_eq!(
            capacity_warnings(&big, &host),
            [
                "--default-cpu-limit 4 is more than the Docker host's 2 CPUs; containers get at most 2",
                "--default-memory-limit 2 GiB is more than the Docker host's memory (1024 MiB)",
            ]
        );
        assert!(capacity_warnings(&big, &ferry_docker::HostInfo::default()).is_empty(), "unknown capacity");
        let no_cfs = ferry_docker::HostInfo { cpu_cfs_quota: Some(false), ..Default::default() };
        let warnings = capacity_warnings(&Config::default(), &no_cfs);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("no CPU CFS quota support"), "{warnings:?}");
    }

    #[test]
    fn oom_score_adj_decision() {
        assert!(should_set_oom_score_adj(Some(0), -500));
        assert!(should_set_oom_score_adj(None, -500));
        assert!(should_set_oom_score_adj(Some(-100), -500));
        assert!(!should_set_oom_score_adj(Some(-500), -500), "already set");
        assert!(!should_set_oom_score_adj(Some(-900), -500), "never raise systemd's OOMScoreAdjust");
        assert!(!should_set_oom_score_adj(Some(0), 0), "0 = leave unchanged");
        assert!(!should_set_oom_score_adj(Some(-900), 0));
        assert!(should_set_oom_score_adj(Some(0), 300), "an explicit positive value is applied");
    }
}
