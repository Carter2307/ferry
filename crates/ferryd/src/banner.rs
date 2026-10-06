//! What the server says when it starts: the lines of the startup banner,
//! and the warnings about limits the Docker host can't honour.

use ferry_core::{Config, resources};

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

/// What the startup banner says under its title, one entry per line.
pub fn summary(
    config: &Config,
    api_url: &str,
    docker_version: Option<&str>,
    docker_host: Option<&ferry_docker::HostInfo>,
) -> Vec<String> {
    let mut lines = vec![format!("Dashboard + API : {api_url}")];
    lines.push(match config.dashboard_url() {
        Some(url) => format!("Dashboard (proxy): {url}"),
        None => format!("Dashboard (proxy): disabled (enable with --dashboard-host ferry.{})", config.base_domain),
    });
    lines.push(format!("Apps            : {}", config.url_for_host(&format!("<name>.{}", config.primary_domain()))));
    lines.push(format!("Docker          : {}", docker_summary(docker_version, docker_host)));
    lines.push(format!("Data            : {}", config.data_dir.display()));
    lines.push(format!("Limits          : {}", limits_summary(config, docker_host)));
    lines.push(match config.min_free_disk_mb {
        0 => "Free disk check : off".to_string(),
        mb => format!(
            "Free disk check : deploys need {} free",
            resources::format_memory_mb(u32::try_from(mb).unwrap_or(u32::MAX))
        ),
    });
    lines
}

/// Warnings about limits the Docker host can't honour.
pub fn capacity_warnings(config: &Config, host: &ferry_docker::HostInfo) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
