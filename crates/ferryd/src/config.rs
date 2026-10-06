//! The server's `Config`, from its command-line options.

use std::path::Path;

use anyhow::Context;
use ferry_core::Config;

use crate::cli::ServerArgs;
use crate::fsutil::{read_private_file, secure_dir, write_private_file};

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

pub fn build_config(args: ServerArgs) -> anyhow::Result<Config> {
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
        base_domain: args.base_domain.trim().trim_end_matches('.').to_ascii_lowercase(),
        domains: Default::default(),
        public_ips: args.public_ip,
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

#[cfg(test)]
mod tests {
    use clap::Parser;
    use ferry_core::resources;

    use super::*;
    use crate::cli::Cli;

    #[test]
    fn config_gets_the_limits() {
        let dir = std::env::temp_dir().join(format!("ferryd-test-{}", ferry_core::ids::random_secret(8)));
        let data_dir = dir.to_string_lossy().into_owned();
        let args = Cli::try_parse_from([
            "ferryd",
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
        .unwrap()
        .server;
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
}
