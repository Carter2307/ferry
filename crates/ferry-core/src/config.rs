//! Server configuration (built by `ferryd` from CLI flags / env vars).

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::models::Service;
use crate::naming::Naming;
use crate::resources::Limits;

/// Runtime configuration of a Ferry server.
#[derive(Debug, Clone)]
pub struct Config {
    /// Root for all state: `ferry.db`, logs, build dirs, uploads, certs.
    pub data_dir: PathBuf,
    /// Control-plane API + dashboard listen address.
    pub api_addr: SocketAddr,
    /// Public HTTP reverse-proxy listen address.
    pub proxy_addr: SocketAddr,
    /// Public HTTPS listen address (only used when TLS is enabled).
    pub proxy_https_addr: Option<SocketAddr>,
    /// Services get `<name>.<base_domain>`. `localhost` works out of the box
    /// because browsers resolve `*.localhost` to 127.0.0.1.
    pub base_domain: String,
    /// Port shown in public URLs. `None` = derived from `proxy_addr`
    /// (omitted when 80, or when TLS is on).
    pub public_port: Option<u16>,
    /// Host routed by the proxy to the dashboard/API (e.g. `ferry.localhost`).
    pub dashboard_host: Option<String>,
    /// Prefix for every Docker resource (containers, images, volumes, network)
    /// and value of the `ferry.instance` label. Lets several Ferry servers
    /// share one Docker daemon without touching each other.
    pub name_prefix: String,
    /// Bearer token required by the API.
    pub api_token: String,
    /// Secret used to verify GitHub `X-Hub-Signature-256` webhook signatures.
    pub github_webhook_secret: Option<String>,
    /// Enables automatic HTTPS (Let's Encrypt, HTTP-01) for custom domains.
    pub acme_email: Option<String>,
    /// Use the Let's Encrypt staging directory.
    pub acme_staging: bool,
    /// Max concurrent image builds.
    pub build_concurrency: usize,
    /// Default container port when nothing else specifies one (Render uses 10000).
    pub default_port: u16,
    /// Deploy images to keep per service for rollbacks.
    pub keep_images: usize,
    /// Finished job runs (and their logs) kept per service.
    pub keep_job_runs: usize,
    /// Path/name of the docker CLI (used for `docker build`).
    pub docker_bin: String,
    /// Seconds a new deploy has to become healthy.
    pub health_check_timeout_secs: u64,
    /// IP that datastore ports are published on (default 127.0.0.1).
    pub datastore_bind_ip: String,
    /// Host name printed in external datastore URLs.
    pub advertise_host: String,
    /// Memory limit (MiB) of service, job and datastore containers that set
    /// none of their own. 0 = unlimited.
    pub default_memory_limit_mb: u32,
    /// CPU limit (CPUs) of service, job and datastore containers that set
    /// none of their own. 0 = unlimited.
    pub default_cpu_limit: f64,
    /// Max processes + threads per container (fork-bomb guard). 0 = unlimited.
    pub pids_limit: u32,
    /// Rotate each container's Docker log (`json-file` driver) when it
    /// reaches this size, in MiB. 0 = leave the daemon's log configuration
    /// alone (no rotation unless the daemon configures it).
    pub log_max_size_mb: u32,
    /// Log files kept per container when rotating (current one included).
    pub log_max_files: u32,
    /// Deploys fail early when less than this much disk (MiB) is free on the
    /// data directory's filesystem (or on the Docker root's, when it is
    /// local). 0 = no check.
    pub min_free_disk_mb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            data_dir: PathBuf::from("./ferry-data"),
            api_addr: "127.0.0.1:7878".parse().unwrap(),
            proxy_addr: "0.0.0.0:8080".parse().unwrap(),
            proxy_https_addr: None,
            base_domain: "localhost".to_string(),
            public_port: None,
            dashboard_host: Some("ferry.localhost".to_string()),
            name_prefix: "ferry".to_string(),
            api_token: String::new(),
            github_webhook_secret: None,
            acme_email: None,
            acme_staging: false,
            build_concurrency: 2,
            default_port: 10000,
            keep_images: 5,
            keep_job_runs: 100,
            docker_bin: "docker".to_string(),
            health_check_timeout_secs: 120,
            datastore_bind_ip: "127.0.0.1".to_string(),
            advertise_host: "127.0.0.1".to_string(),
            default_memory_limit_mb: 512,
            default_cpu_limit: 1.0,
            pids_limit: 1024,
            log_max_size_mb: 10,
            log_max_files: 3,
            min_free_disk_mb: 1024,
        }
    }
}

impl Config {
    /// TLS is on when an ACME email and an HTTPS address are configured.
    pub fn tls_enabled(&self) -> bool {
        self.acme_email.is_some() && self.proxy_https_addr.is_some()
    }

    /// Effective limits of a container whose service or datastore sets
    /// `memory_mb` / `cpus` (`None` = use the server defaults; a default of 0
    /// = unlimited).
    pub fn limits(&self, memory_mb: Option<u32>, cpus: Option<f64>) -> Limits {
        Limits {
            memory_mb: memory_mb.filter(|m| *m > 0).or(Some(self.default_memory_limit_mb).filter(|m| *m > 0)),
            cpus: cpus
                .filter(|c| c.is_finite() && *c > 0.0)
                .or(Some(self.default_cpu_limit).filter(|c| c.is_finite() && *c > 0.0)),
        }
    }

    pub fn naming(&self) -> Naming {
        Naming::new(&self.name_prefix)
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("ferry.db")
    }
    /// Deploy / job logs: `logs/deploys/<id>.log`, `logs/jobs/<id>.log` (JSON lines).
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    /// Scratch build contexts: `builds/<deploy_id>/`.
    pub fn builds_dir(&self) -> PathBuf {
        self.data_dir.join("builds")
    }
    /// Git mirrors: `repos/<service_id>/`.
    pub fn repos_dir(&self) -> PathBuf {
        self.data_dir.join("repos")
    }
    /// Uploaded source archives: `uploads/<id>.tar.gz`.
    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }
    /// ACME account + certificates.
    pub fn certs_dir(&self) -> PathBuf {
        self.data_dir.join("certs")
    }

    /// Default hostname of a service: `<name>.<base_domain>`.
    pub fn default_host(&self, service_name: &str) -> String {
        format!("{}.{}", service_name, self.base_domain).to_ascii_lowercase()
    }

    /// All hostnames the proxy routes to this service (empty unless public HTTP).
    pub fn service_hosts(&self, service: &Service) -> Vec<String> {
        if !service.is_public_http() {
            return Vec::new();
        }
        let mut hosts = vec![self.default_host(&service.name)];
        for d in &service.custom_domains {
            let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
            if !d.is_empty() && !hosts.contains(&d) {
                hosts.push(d);
            }
        }
        hosts
    }

    /// Scheme + optional port suffix used in public URLs.
    fn public_scheme_and_port(&self) -> (&'static str, String) {
        if self.tls_enabled() {
            return ("https", String::new());
        }
        let port = self.public_port.unwrap_or(self.proxy_addr.port());
        let suffix = if port == 80 { String::new() } else { format!(":{port}") };
        ("http", suffix)
    }

    /// Public URL for a hostname served by the proxy.
    pub fn url_for_host(&self, host: &str) -> String {
        let (scheme, port) = self.public_scheme_and_port();
        // Local hostnames never get certificates, so keep them on http.
        if scheme == "https" && is_local_host(host) {
            let port = self.proxy_addr.port();
            let suffix = if port == 80 { String::new() } else { format!(":{port}") };
            return format!("http://{host}{suffix}");
        }
        format!("{scheme}://{host}{port}")
    }

    /// Public URL of a service (web services and static sites only).
    pub fn service_url(&self, service: &Service) -> Option<String> {
        service.is_public_http().then(|| self.url_for_host(&self.default_host(&service.name)))
    }

    /// URL of the dashboard through the proxy, if a dashboard host is configured.
    pub fn dashboard_url(&self) -> Option<String> {
        self.dashboard_host.as_deref().map(|h| self.url_for_host(h))
    }
}

/// Hosts that can never get a public certificate.
pub fn is_local_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".internal")
        || h.ends_with(".test")
        || h.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServiceType;

    #[test]
    fn urls_and_hosts() {
        let cfg = Config::default();
        let mut svc = Service::new("web", ServiceType::WebService);
        svc.custom_domains = vec!["App.Example.com.".into(), "web.localhost".into()];
        assert_eq!(cfg.service_hosts(&svc), vec!["web.localhost", "app.example.com"]);
        assert_eq!(cfg.service_url(&svc).unwrap(), "http://web.localhost:8080");
        let worker = Service::new("w", ServiceType::BackgroundWorker);
        assert!(cfg.service_hosts(&worker).is_empty());
        assert!(cfg.service_url(&worker).is_none());
    }

    #[test]
    fn effective_limits() {
        let cfg = Config::default();
        assert_eq!(cfg.limits(None, None), Limits { memory_mb: Some(512), cpus: Some(1.0) });
        assert_eq!(cfg.limits(Some(2048), Some(0.5)), Limits { memory_mb: Some(2048), cpus: Some(0.5) });
        let unlimited = Config { default_memory_limit_mb: 0, default_cpu_limit: 0.0, ..Config::default() };
        assert_eq!(unlimited.limits(None, None), Limits::default());
        assert_eq!(unlimited.limits(Some(256), None), Limits { memory_mb: Some(256), cpus: None });
    }

    #[test]
    fn tls_urls() {
        let cfg = Config {
            acme_email: Some("a@b.c".into()),
            proxy_https_addr: Some("0.0.0.0:443".parse().unwrap()),
            proxy_addr: "0.0.0.0:80".parse().unwrap(),
            base_domain: "apps.example.com".into(),
            ..Config::default()
        };
        assert_eq!(cfg.url_for_host("web.apps.example.com"), "https://web.apps.example.com");
        assert_eq!(cfg.url_for_host("web.localhost"), "http://web.localhost");
    }
}
