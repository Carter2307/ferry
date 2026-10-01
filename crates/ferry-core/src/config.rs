//! Server configuration (built by `ferryd` from CLI flags / env vars).

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use crate::models::{Domain, Service};
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
    /// because browsers resolve `*.localhost` to 127.0.0.1. More domains are
    /// connected while the server runs: see `domains`.
    pub base_domain: String,
    /// The domains services are served under, as the running server knows
    /// them (DESIGN.md §21). `base_domain` alone until the store was read.
    pub domains: ServedDomains,
    /// The addresses this server is reached at from the internet
    /// (`--public-ip`), for the DNS records of a domain. Empty = found out
    /// by the engine.
    pub public_ips: Vec<IpAddr>,
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
            domains: ServedDomains::default(),
            public_ips: Vec::new(),
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

    /// The domain of a service's URL: the default domain while it is served,
    /// else the base domain.
    pub fn primary_domain(&self) -> String {
        self.domains.read().map_or_else(|| self.base_domain.to_ascii_lowercase(), |s| s.primary)
    }

    /// Every domain services are served under, the primary one first.
    pub fn served_domains(&self) -> Vec<String> {
        self.domains.read().map_or_else(|| vec![self.base_domain.to_ascii_lowercase()], |s| s.served)
    }

    /// The served domains plus the ones still waiting for their DNS: the
    /// hostnames under all of them are spoken for.
    pub fn claimed_domains(&self) -> Vec<String> {
        self.domains.read().map_or_else(|| vec![self.base_domain.to_ascii_lowercase()], |s| s.claimed)
    }

    /// Default hostname of a service: `<name>.<primary domain>`.
    pub fn default_host(&self, service_name: &str) -> String {
        format!("{}.{}", service_name, self.primary_domain()).to_ascii_lowercase()
    }

    /// The hostnames of a service under every served domain, the default
    /// host first.
    pub fn default_hosts(&self, service_name: &str) -> Vec<String> {
        self.served_domains().iter().map(|d| format!("{service_name}.{d}").to_ascii_lowercase()).collect()
    }

    /// [`default_hosts`](Self::default_hosts) plus the hostnames under the
    /// domains that are not served yet: no other service may take them.
    pub fn claimed_hosts(&self, service_name: &str) -> Vec<String> {
        self.claimed_domains().iter().map(|d| format!("{service_name}.{d}").to_ascii_lowercase()).collect()
    }

    /// All hostnames the proxy routes to this service (empty unless public HTTP).
    pub fn service_hosts(&self, service: &Service) -> Vec<String> {
        if !service.is_public_http() {
            return Vec::new();
        }
        let mut hosts = self.default_hosts(&service.name);
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

/// The domains services are served under, shared by everything that holds
/// the server's [`Config`] — its clones share it too: the API, the engine,
/// the environment of a deploy.
///
/// It starts empty, which means "the base domain alone": the whole story for
/// a `Config` that never sees a store. `domains::reload` replaces it with
/// what the store holds whenever a domain is connected, verified, made the
/// default or removed (DESIGN.md §21).
#[derive(Debug, Clone)]
pub struct ServedDomains {
    state: Arc<RwLock<Option<Served>>>,
    /// What this server answers a domain verification with (`domains::probe_answer`).
    probe_id: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Served {
    primary: String,
    served: Vec<String>,
    claimed: Vec<String>,
}

impl Default for ServedDomains {
    fn default() -> Self {
        ServedDomains { state: Arc::default(), probe_id: crate::ids::random_secret(32).into() }
    }
}

impl ServedDomains {
    /// A value made up when the server starts: a verification request that
    /// comes back with it reached this very process.
    pub fn probe_id(&self) -> &str {
        &self.probe_id
    }

    fn read(&self) -> Option<Served> {
        // A poisoned lock still holds a valid value (it is replaced whole).
        self.state.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Replace the set with the rows of the store. The base domain is served
    /// whatever its row says (or without one); the default domain is the
    /// primary one while it is served. Returns whether anything changed.
    pub fn replace(&self, base_domain: &str, domains: &[Domain]) -> bool {
        fn push(list: &mut Vec<String>, name: &str) {
            if !list.iter().any(|n| n == name) {
                list.push(name.to_string());
            }
        }
        let base = base_domain.to_ascii_lowercase();
        let (mut served, mut claimed, mut primary) = (Vec::new(), Vec::new(), None);
        for d in domains {
            push(&mut claimed, &d.name);
            if d.is_served() || d.name == base {
                push(&mut served, &d.name);
                if d.is_default {
                    primary = Some(d.name.clone());
                }
            }
        }
        push(&mut served, &base);
        push(&mut claimed, &base);
        let primary = primary.unwrap_or(base);
        // The primary first (the others keep their order): a service's first
        // host is the one of its URL.
        served.sort_by_key(|name| *name != primary);
        let next = Served { primary, served, claimed };
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if state.as_ref() == Some(&next) {
            return false;
        }
        *state = Some(next);
        true
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
    fn hosts_follow_the_served_domains() {
        use crate::models::{Domain, DomainSource, DomainStatus};
        let cfg = Config::default();
        let web = Service::new("web", ServiceType::WebService);
        // Before the store is read: the base domain alone.
        assert_eq!(cfg.primary_domain(), "localhost");
        assert_eq!(cfg.claimed_hosts("web"), vec!["web.localhost"]);

        let base = Domain { is_default: true, ..Domain::new("localhost", DomainSource::Config) };
        let pending = Domain::new("example.com", DomainSource::Connected);
        assert!(cfg.domains.replace("localhost", &[base.clone(), pending.clone()]));
        // Unchanged rows change nothing.
        assert!(!cfg.domains.replace("localhost", &[base.clone(), pending.clone()]));
        // A pending domain is not served, but its names are spoken for.
        assert_eq!(cfg.service_hosts(&web), vec!["web.localhost"]);
        assert_eq!(cfg.claimed_hosts("web"), vec!["web.localhost", "web.example.com"]);

        // Verified: served next to the base domain, which is still the default.
        let active = Domain { status: DomainStatus::Active, ..pending.clone() };
        assert!(cfg.domains.replace("localhost", &[base.clone(), active.clone()]));
        assert_eq!(cfg.service_hosts(&web), vec!["web.localhost", "web.example.com"]);
        assert_eq!(cfg.service_url(&web).unwrap(), "http://web.localhost:8080");

        // The default domain is the one of the URL, and comes first.
        let base = Domain { is_default: false, ..base };
        let default = Domain { is_default: true, ..active };
        assert!(cfg.domains.replace("localhost", &[default.clone(), base.clone()]));
        assert_eq!(cfg.primary_domain(), "example.com");
        assert_eq!(cfg.service_hosts(&web), vec!["web.example.com", "web.localhost"]);
        assert_eq!(cfg.service_url(&web).unwrap(), "http://web.example.com:8080");
        // A clone of the configuration sees the same domains.
        assert_eq!(cfg.clone().default_host("api"), "api.example.com");

        // It stays served when its DNS breaks later...
        let broken = Domain { status: DomainStatus::Misconfigured, ..default.clone() };
        cfg.domains.replace("localhost", &[broken, base.clone()]);
        assert_eq!(cfg.default_host("web"), "web.example.com");
        // ...and a default domain that is not served is not the primary one.
        let unverified = Domain { status: DomainStatus::Pending, ..default };
        cfg.domains.replace("localhost", &[unverified, base]);
        assert_eq!(cfg.primary_domain(), "localhost");
        // The base domain is served even without its row.
        cfg.domains.replace("localhost", &[]);
        assert_eq!(cfg.served_domains(), vec!["localhost"]);
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
