//! Resources of the containers Ferry creates (service instances, one-off
//! jobs, cron runs, datastores): memory and CPU limits (the service's or
//! datastore's own, else the server defaults — see
//! [`ferry_core::Config::limits`]), the pids limit and log rotation. Also the
//! messages for containers the kernel OOM-killed, and the free-disk check
//! made before deploys build and datastores are created.
//!
//! A CPU limit above the Docker host's CPU count makes Docker refuse to
//! create the container, so it is capped at the host's CPUs (the host's
//! capacity is read once, see [`Inner::host_info`]); on a host without CPU
//! CFS quotas Docker refuses any CPU limit, so none is set (a warning). A
//! memory limit above the host's memory is allowed (it just never protects
//! anything): a warning. Log rotation (`json-file`) is only set when the
//! daemon's own log driver is `json-file`: another one (journald, a log
//! shipper, `local`) is the operator's choice and is kept.

use std::path::{Path, PathBuf};

use ferry_core::resources::{Limits, format_cpus, format_memory_mb};
use ferry_core::{Config, Error, Result};
use ferry_docker::{ContainerSpec, HostInfo, LimitsUpdate, LogRotation};
use tracing::debug;

use crate::state::Inner;

const MIB: i64 = 1024 * 1024;

/// What one container is created with (or updated to).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Resources {
    /// Effective memory / CPU limits (CPU already capped). `None` = unlimited.
    pub limits: Limits,
    pub pids_limit: Option<i64>,
    pub log_rotation: Option<LogRotation>,
    /// The CPU limit asked for, when it was capped at the host's CPU count.
    pub cpus_capped_from: Option<f64>,
    /// The CPU limit asked for, when the host can't enforce CPU limits (no
    /// CPU CFS quotas: Docker would refuse the container), so none is set.
    pub cpus_unsupported: Option<f64>,
    /// The Docker host's memory (bytes), when the memory limit exceeds it.
    pub host_memory_below_limit: Option<u64>,
}

impl Resources {
    /// The resources of a container whose service / datastore sets
    /// `memory_mb` / `cpus` (`None` = the server default). `host` is the
    /// Docker host's capacity, if known (no capping otherwise).
    pub(crate) fn new(config: &Config, host: Option<&HostInfo>, memory_mb: Option<u32>, cpus: Option<f64>) -> Self {
        let mut limits = config.limits(memory_mb, cpus);
        let mut cpus_capped_from = None;
        let mut cpus_unsupported = None;
        if host.and_then(|h| h.cpu_cfs_quota) == Some(false) {
            cpus_unsupported = limits.cpus.take();
        }
        if let (Some(requested), Some(host_cpus)) = (limits.cpus, host.and_then(|h| h.cpus))
            && requested > f64::from(host_cpus)
        {
            cpus_capped_from = Some(requested);
            limits.cpus = Some(f64::from(host_cpus));
        }
        let host_memory_below_limit = match (limits.memory_bytes(), host.and_then(|h| h.memory_bytes)) {
            (Some(limit), Some(host_bytes)) if u64::try_from(limit).is_ok_and(|l| l > host_bytes) => Some(host_bytes),
            _ => None,
        };
        Resources {
            limits,
            pids_limit: Some(i64::from(config.pids_limit)).filter(|p| *p > 0),
            log_rotation: (config.log_max_size_mb > 0 && rotates_json_file(host))
                .then(|| LogRotation { max_size_mb: config.log_max_size_mb, max_files: config.log_max_files.max(1) }),
            cpus_capped_from,
            cpus_unsupported,
            host_memory_below_limit,
        }
    }

    /// Set the container spec's limits, pids limit and log rotation.
    pub(crate) fn apply(&self, spec: &mut ContainerSpec) {
        spec.memory_limit_bytes = self.limits.memory_bytes();
        spec.nano_cpus = self.limits.nano_cpus();
        spec.pids_limit = self.pids_limit;
        spec.log_rotation = self.log_rotation;
    }

    /// The same limits for `docker update` (a running container).
    pub(crate) fn update(&self) -> LimitsUpdate {
        LimitsUpdate {
            memory_limit_bytes: self.limits.memory_bytes(),
            nano_cpus: self.limits.nano_cpus(),
            pids_limit: self.pids_limit,
        }
    }

    /// `512 MiB memory, 1 CPU` (`unlimited memory`, `unlimited CPU`).
    pub(crate) fn summary(&self) -> String {
        let memory = match self.limits.memory_mb.filter(|m| *m > 0) {
            Some(mb) => format!("{} memory", format_memory_mb(mb)),
            None => "unlimited memory".to_string(),
        };
        let cpu = match self.limits.cpus.filter(|c| *c > 0.0) {
            Some(c) => format_cpus(c),
            None => "unlimited CPU".to_string(),
        };
        format!("{memory}, {cpu}")
    }

    /// The CPU limit was capped at the host's CPU count (for the deploy log
    /// and the server log).
    pub(crate) fn cpu_cap_warning(&self) -> Option<String> {
        let (requested, capped) = (self.cpus_capped_from?, self.limits.cpus?);
        Some(format!(
            "the CPU limit ({}) is more than the Docker host has: capped at {}",
            format_cpus(requested),
            format_cpus(capped)
        ))
    }

    /// The host can't enforce the CPU limit (for the deploy log).
    pub(crate) fn cpu_unsupported_warning(&self) -> Option<String> {
        let requested = self.cpus_unsupported?;
        Some(format!(
            "the CPU limit ({}) is not enforced: the Docker host's kernel has no CPU CFS quota support",
            format_cpus(requested)
        ))
    }

    /// The memory limit is above the host's memory: it protects nothing.
    pub(crate) fn memory_warning(&self) -> Option<String> {
        let (host, mb) = (self.host_memory_below_limit?, self.limits.memory_mb?);
        Some(format!(
            "the memory limit ({}) is more than the Docker host's memory ({}): it won't stop the container from \
             exhausting the host's memory",
            format_memory_mb(mb),
            human_mib(host / MIB as u64)
        ))
    }

    /// Every warning that applies.
    pub(crate) fn warnings(&self) -> Vec<String> {
        self.cpu_cap_warning().into_iter().chain(self.cpu_unsupported_warning()).chain(self.memory_warning()).collect()
    }
}

/// Whether Ferry's `json-file` log rotation applies: the daemon's default
/// log driver is `json-file` (or unknown). Any other driver was chosen by
/// the operator (journald, syslog, a log shipper; `local` rotates on its
/// own) and is kept.
pub(crate) fn rotates_json_file(host: Option<&HostInfo>) -> bool {
    host.and_then(|h| h.logging_driver.as_deref()).is_none_or(|driver| driver == "json-file")
}

/// The resources of a container Ferry creates now (see [`Resources::new`]).
pub(crate) async fn for_container(inner: &Inner, memory_mb: Option<u32>, cpus: Option<f64>) -> Resources {
    let host = inner.host_info().await;
    Resources::new(&inner.config, host, memory_mb, cpus)
}

/// `512 MiB`, `1.5 GiB`, `7.7 GiB` (one decimal above 1 GiB).
pub(crate) fn human_mib(mb: u64) -> String {
    if mb < 1024 {
        return format!("{mb} MiB");
    }
    let gib = format!("{:.1}", mb as f64 / 1024.0);
    format!("{} GiB", gib.trim_end_matches(".0"))
}

/// The memory limit a container had, for messages: `512 MiB`.
fn limit_text(limit_bytes: i64) -> String {
    let mb = (limit_bytes + MIB / 2) / MIB;
    format_memory_mb(u32::try_from(mb.max(1)).unwrap_or(u32::MAX))
}

/// Why the kernel killed `subject` ("instance abc123", "the job", "the
/// redis container"): its memory limit, whose owner (`"the service's"`,
/// `"the datastore's"`) should raise it. Without a limit, the host itself ran
/// out of memory.
pub(crate) fn oom_message(subject: &str, limit_bytes: Option<i64>, owner: &str) -> String {
    match limit_bytes.filter(|l| *l > 0) {
        Some(limit) => {
            format!("{subject} ran out of memory (limit {}) — raise {owner} memory limit", limit_text(limit))
        }
        None => format!(
            "{subject} was killed by the kernel's out-of-memory killer (it has no memory limit: the Docker host ran \
             out of memory)"
        ),
    }
}

// ---------------------------------------------------------------------------
// free disk space

/// Free space (bytes) available to unprivileged users on the filesystem of
/// `path`.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast, clippy::useless_conversion)]
pub(crate) fn free_disk_bytes(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "the path contains a NUL byte"))?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid NUL-terminated string and `st` points to
    // writable memory the size of a `statvfs`, filled in on success.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statvfs succeeded, so the struct is initialized.
    let st = unsafe { st.assume_init() };
    // The field types differ between platforms (u32 / u64).
    Ok(u64::from(st.f_bavail).saturating_mul(st.f_frsize as u64))
}

#[cfg(not(unix))]
pub(crate) fn free_disk_bytes(_path: &Path) -> std::io::Result<u64> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "free disk space is only checked on Unix"))
}

/// The error when `path` has only `free_bytes` left, below `min_mb`.
pub(crate) fn low_disk_message(path: &Path, free_bytes: u64, min_mb: u64) -> String {
    format!(
        "not enough free disk space on {}: {} free, Ferry needs at least {} (free some space, e.g. remove unused \
         Docker images with `docker image prune`, or change the threshold with `ferryd --min-free-disk`, 0 = off)",
        path.display(),
        human_mib(free_bytes / MIB as u64),
        human_mib(min_mb)
    )
}

/// The first of `paths` whose filesystem has less than `min_mb` free.
/// Paths that can't be checked are skipped.
fn first_low(paths: &[PathBuf], min_mb: u64) -> Option<String> {
    let min_bytes = min_mb.saturating_mul(MIB as u64);
    for path in paths {
        match free_disk_bytes(path) {
            Ok(free) if free < min_bytes => return Some(low_disk_message(path, free, min_mb)),
            Ok(_) => {}
            Err(e) => debug!(path = %path.display(), "cannot check free disk space: {e}"),
        }
    }
    None
}

/// The Docker root dir, when it is on this machine's filesystem: not on
/// Docker Desktop (it lives in a VM there) nor on a remote daemon.
pub(crate) fn local_docker_root(host: Option<&HostInfo>) -> Option<PathBuf> {
    let host = host?;
    if host.operating_system.as_deref().is_some_and(|os| os.eq_ignore_ascii_case("Docker Desktop")) {
        return None;
    }
    let root = PathBuf::from(host.docker_root_dir.as_deref()?);
    root.is_dir().then_some(root)
}

/// Fail early (before a build, a pull or a new datastore container) when the
/// data directory's filesystem — or the Docker root dir's, when it is local —
/// has less than `config.min_free_disk_mb` free (0 = no check).
pub(crate) async fn check_free_disk(inner: &Inner) -> Result<()> {
    let min_mb = inner.config.min_free_disk_mb;
    if min_mb == 0 {
        return Ok(());
    }
    let mut paths = vec![inner.config.data_dir.clone()];
    if let Some(root) = local_docker_root(inner.host_info().await) {
        paths.push(root);
    }
    // statvfs may block (e.g. on a network filesystem).
    match tokio::task::spawn_blocking(move || first_low(&paths, min_mb)).await {
        Ok(Some(message)) => Err(Error::Build(message)),
        Ok(None) => Ok(()),
        Err(e) => {
            debug!("free disk space check failed: {e}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(cpus: u32, memory_gib: u64) -> HostInfo {
        HostInfo {
            cpus: Some(cpus),
            memory_bytes: Some(memory_gib << 30),
            docker_root_dir: Some("/var/lib/docker".into()),
            operating_system: Some("Ubuntu 24.04 LTS".into()),
            cpu_cfs_quota: Some(true),
            logging_driver: Some("json-file".into()),
        }
    }

    #[test]
    fn defaults_explicit_values_pids_and_log_rotation() {
        let config = Config::default();
        let r = Resources::new(&config, None, None, None);
        assert_eq!(r.limits, Limits { memory_mb: Some(512), cpus: Some(1.0) });
        assert_eq!(r.pids_limit, Some(1024));
        assert_eq!(r.log_rotation, Some(LogRotation { max_size_mb: 10, max_files: 3 }));
        assert_eq!(r.summary(), "512 MiB memory, 1 CPU");
        assert!(r.warnings().is_empty());

        let r = Resources::new(&config, Some(&host(4, 8)), Some(2048), Some(0.5));
        assert_eq!(r.limits, Limits { memory_mb: Some(2048), cpus: Some(0.5) });
        assert_eq!(r.summary(), "2 GiB memory, 0.5 CPU");

        let mut spec = ContainerSpec::default();
        r.apply(&mut spec);
        assert_eq!(spec.memory_limit_bytes, Some(2048 << 20));
        assert_eq!(spec.nano_cpus, Some(500_000_000));
        assert_eq!(spec.pids_limit, Some(1024));
        assert_eq!(spec.log_rotation, Some(LogRotation { max_size_mb: 10, max_files: 3 }));
        assert_eq!(
            r.update(),
            LimitsUpdate { memory_limit_bytes: Some(2048 << 20), nano_cpus: Some(500_000_000), pids_limit: Some(1024) }
        );

        // The operator turned everything off.
        let off = Config {
            default_memory_limit_mb: 0,
            default_cpu_limit: 0.0,
            pids_limit: 0,
            log_max_size_mb: 0,
            ..Config::default()
        };
        let r = Resources::new(&off, Some(&host(4, 8)), None, None);
        assert_eq!(r, Resources::default());
        assert_eq!(r.summary(), "unlimited memory, unlimited CPU");
        let mut spec = ContainerSpec { pids_limit: Some(1), ..ContainerSpec::default() };
        r.apply(&mut spec);
        assert_eq!(
            (spec.memory_limit_bytes, spec.nano_cpus, spec.pids_limit, spec.log_rotation),
            (None, None, None, None)
        );
        // At least one rotated file.
        let one = Config { log_max_files: 0, ..Config::default() };
        assert_eq!(Resources::new(&one, None, None, None).log_rotation.unwrap().max_files, 1);
    }

    #[test]
    fn cpus_are_capped_at_the_host_and_big_memory_is_a_warning() {
        let config = Config::default();
        let r = Resources::new(&config, Some(&host(4, 8)), Some(16 * 1024), Some(8.0));
        assert_eq!(r.limits.cpus, Some(4.0));
        assert_eq!(r.limits.nano_cpus(), Some(4_000_000_000));
        assert_eq!(r.cpus_capped_from, Some(8.0));
        assert_eq!(r.host_memory_below_limit, Some(8 << 30));
        assert_eq!(r.limits.memory_mb, Some(16 * 1024), "memory is never capped");
        let w = r.warnings();
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[0], "the CPU limit (8 CPUs) is more than the Docker host has: capped at 4 CPUs");
        assert!(w[1].starts_with("the memory limit (16 GiB) is more than the Docker host's memory (8 GiB)"), "{w:?}");
        assert_eq!(r.summary(), "16 GiB memory, 4 CPUs");
        // Exactly the host's CPUs is fine; unknown host capacity: no capping.
        assert_eq!(Resources::new(&config, Some(&host(4, 8)), None, Some(4.0)).cpus_capped_from, None);
        assert_eq!(Resources::new(&config, None, None, Some(64.0)).limits.cpus, Some(64.0));
        let unknown = HostInfo::default();
        assert_eq!(Resources::new(&config, Some(&unknown), Some(1 << 20), Some(64.0)).warnings(), Vec::<String>::new());
    }

    #[test]
    fn no_cpu_limit_without_cfs_quotas() {
        let config = Config::default();
        let mut h = host(4, 8);
        h.cpu_cfs_quota = Some(false);
        let r = Resources::new(&config, Some(&h), Some(256), Some(0.5));
        assert_eq!(r.limits, Limits { memory_mb: Some(256), cpus: None }, "Docker would refuse NanoCpus");
        assert_eq!(r.cpus_unsupported, Some(0.5));
        assert_eq!(r.update().nano_cpus, None);
        assert_eq!(r.summary(), "256 MiB memory, unlimited CPU");
        assert_eq!(
            r.warnings(),
            vec!["the CPU limit (0.5 CPU) is not enforced: the Docker host's kernel has no CPU CFS quota support"]
        );
        let mut spec = ContainerSpec::default();
        r.apply(&mut spec);
        assert_eq!((spec.memory_limit_bytes, spec.nano_cpus), (Some(256 << 20), None));
        // The server default too; nothing to warn about when unlimited.
        assert_eq!(Resources::new(&config, Some(&h), None, None).cpus_unsupported, Some(1.0));
        let off = Config { default_cpu_limit: 0.0, ..Config::default() };
        assert!(Resources::new(&off, Some(&h), None, None).warnings().is_empty());
        // Not reported: assumed supported.
        h.cpu_cfs_quota = None;
        assert_eq!(Resources::new(&config, Some(&h), None, Some(0.5)).limits.cpus, Some(0.5));
    }

    #[test]
    fn log_rotation_only_replaces_json_file() {
        let config = Config::default();
        let rotation = Some(LogRotation { max_size_mb: 10, max_files: 3 });
        let mut h = host(4, 8);
        assert_eq!(Resources::new(&config, Some(&h), None, None).log_rotation, rotation);
        assert_eq!(Resources::new(&config, None, None, None).log_rotation, rotation, "unknown driver");
        for driver in ["journald", "local", "fluentd", "syslog"] {
            h.logging_driver = Some(driver.into());
            assert_eq!(Resources::new(&config, Some(&h), None, None).log_rotation, None, "{driver} is kept");
        }
        h.logging_driver = None;
        assert!(rotates_json_file(Some(&h)));
    }

    #[test]
    fn oom_messages_name_the_limit() {
        assert_eq!(
            oom_message("instance abc123", Some(512 << 20), "the service's"),
            "instance abc123 ran out of memory (limit 512 MiB) — raise the service's memory limit"
        );
        assert_eq!(
            oom_message("the redis container", Some(1 << 30), "the datastore's"),
            "the redis container ran out of memory (limit 1 GiB) — raise the datastore's memory limit"
        );
        let m = oom_message("the job", None, "the service's");
        assert!(m.starts_with("the job was killed by the kernel's out-of-memory killer"), "{m}");
        assert!(m.contains("no memory limit"), "{m}");
        assert_eq!(human_mib(812), "812 MiB");
        assert_eq!(human_mib(1024), "1 GiB");
        assert_eq!(human_mib(7836), "7.7 GiB");
    }

    #[test]
    fn free_disk_space_is_measured_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        if cfg!(unix) {
            let free = free_disk_bytes(dir.path()).unwrap();
            assert!(free > 0);
            // Nothing has a petabyte free; everything has more than 0 bytes.
            let huge = 1u64 << 30; // MiB = 1 PiB
            let m = first_low(&[dir.path().to_path_buf()], huge).unwrap();
            assert!(m.contains(&dir.path().display().to_string()), "{m}");
            assert!(m.contains("ferryd --min-free-disk"), "{m}");
            assert_eq!(first_low(&[dir.path().to_path_buf()], 1), None);
            assert!(free_disk_bytes(&dir.path().join("missing")).is_err());
        }
        // Paths that can't be checked are skipped.
        assert_eq!(first_low(&[dir.path().join("missing")], 1u64 << 30), None);
        let m = low_disk_message(Path::new("/srv/ferry"), 812 << 20, 1024);
        assert_eq!(
            m,
            "not enough free disk space on /srv/ferry: 812 MiB free, Ferry needs at least 1 GiB (free some space, \
             e.g. remove unused Docker images with `docker image prune`, or change the threshold with `ferryd \
             --min-free-disk`, 0 = off)"
        );
    }

    #[test]
    fn docker_root_is_checked_only_when_local() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = host(4, 8);
        h.docker_root_dir = Some(dir.path().display().to_string());
        assert_eq!(local_docker_root(Some(&h)), Some(dir.path().to_path_buf()));
        h.operating_system = Some("Docker Desktop".into());
        assert_eq!(local_docker_root(Some(&h)), None, "inside Docker Desktop's VM");
        h.operating_system = Some("Debian GNU/Linux 12 (bookworm)".into());
        h.docker_root_dir = Some(dir.path().join("elsewhere").display().to_string());
        assert_eq!(local_docker_root(Some(&h)), None, "a remote daemon's path");
        assert_eq!(local_docker_root(None), None);
    }
}
