//! # ferry-docker
//!
//! Thin, typed wrapper over the Docker Engine API (via `bollard`). The engine
//! uses it to run, inspect, stop and stream logs of containers; to manage
//! volumes, networks and images.
//!
//! Rules:
//! * All "remove"/"stop" operations are idempotent: a missing object is `Ok(())`.
//! * Errors map to `ferry_core::Error::Docker` (or `NotFound` / `Conflict` where noted).
//! * Nothing here knows about services or deploys — naming/labels come from the caller.

use std::collections::BTreeMap;

use ferry_core::{LogSink, LogStream, Result};

/// Docker restart policy for a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartPolicy {
    /// Never restart (jobs).
    #[default]
    No,
    /// Restart unless explicitly stopped (services, datastores).
    UnlessStopped,
    /// Restart on non-zero exit.
    OnFailure,
}

/// Publish one container TCP port on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortPublish {
    pub container_port: u16,
    /// Host IP to bind (Ferry uses `127.0.0.1`: only the proxy / host can reach it).
    pub host_ip: String,
    /// Fixed host port, or `None` for an ephemeral port chosen by Docker.
    pub host_port: Option<u16>,
}

/// Mount a named volume into the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeMount {
    pub volume: String,
    pub target: String,
}

/// Everything needed to create a container.
#[derive(Debug, Clone, Default)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub env: Vec<(String, String)>,
    /// Overrides the image CMD.
    pub cmd: Option<Vec<String>>,
    /// Overrides the image ENTRYPOINT.
    pub entrypoint: Option<Vec<String>>,
    pub labels: BTreeMap<String, String>,
    /// Network to attach to (user-defined bridge).
    pub network: Option<String>,
    /// DNS aliases on `network` (e.g. the service name for private networking).
    pub network_aliases: Vec<String>,
    pub publish: Option<PortPublish>,
    pub volumes: Vec<VolumeMount>,
    pub restart_policy: RestartPolicy,
    pub memory_limit_bytes: Option<i64>,
    /// CPU quota in units of 1e-9 CPUs.
    pub nano_cpus: Option<i64>,
    pub working_dir: Option<String>,
}

/// Docker container state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    Unknown,
}

impl ContainerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContainerState::Created => "created",
            ContainerState::Running => "running",
            ContainerState::Paused => "paused",
            ContainerState::Restarting => "restarting",
            ContainerState::Removing => "removing",
            ContainerState::Exited => "exited",
            ContainerState::Dead => "dead",
            ContainerState::Unknown => "unknown",
        }
    }

    /// Parse Docker's state string (`running`, `exited`, ...).
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "created" => ContainerState::Created,
            "running" => ContainerState::Running,
            "paused" => ContainerState::Paused,
            "restarting" => ContainerState::Restarting,
            "removing" => ContainerState::Removing,
            "exited" => ContainerState::Exited,
            "dead" => ContainerState::Dead,
            _ => ContainerState::Unknown,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, ContainerState::Running)
    }
}

/// What the engine needs to know about a container.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerInfo {
    pub id: String,
    /// Name without Docker's leading `/`.
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    /// Exit code once exited.
    pub exit_code: Option<i64>,
    /// Host port bound for the container's published TCP port (if any).
    pub host_port: Option<u16>,
    pub labels: BTreeMap<String, String>,
    /// RFC3339 start time.
    pub started_at: Option<String>,
    pub restart_count: Option<i64>,
}

/// One-shot resource usage.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ContainerStats {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
}

/// Result of `exec`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecOutput {
    pub exit_code: i64,
    /// stdout and stderr interleaved.
    pub output: String,
}

/// Cloneable Docker client.
#[derive(Clone)]
pub struct Docker {
    inner: bollard::Docker,
}

impl std::fmt::Debug for Docker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Docker").finish_non_exhaustive()
    }
}

impl Docker {
    /// Connect using local defaults (honours `DOCKER_HOST`; on macOS also tries
    /// the Docker Desktop socket `~/.docker/run/docker.sock`) and ping.
    pub async fn connect() -> Result<Self> {
        let _ = &bollard::Docker::connect_with_local_defaults;
        todo!("ferry-docker: connect")
    }

    /// Wrap an existing bollard client.
    pub fn from_bollard(inner: bollard::Docker) -> Self {
        Docker { inner }
    }

    /// Access the underlying bollard client (escape hatch).
    pub fn bollard(&self) -> &bollard::Docker {
        &self.inner
    }

    /// Docker server version string, e.g. `29.2.0`.
    pub async fn version(&self) -> Result<String> {
        todo!("ferry-docker: version")
    }

    /// Create a bridge network if it doesn't exist.
    pub async fn ensure_network(&self, name: &str, labels: &BTreeMap<String, String>) -> Result<()> {
        let _ = (name, labels);
        todo!("ferry-docker: ensure_network")
    }

    /// Create a named volume if it doesn't exist.
    pub async fn ensure_volume(&self, name: &str, labels: &BTreeMap<String, String>) -> Result<()> {
        let _ = (name, labels);
        todo!("ferry-docker: ensure_volume")
    }

    /// Remove a named volume (missing = Ok).
    pub async fn remove_volume(&self, name: &str) -> Result<()> {
        let _ = name;
        todo!("ferry-docker: remove_volume")
    }

    /// True if the image exists locally.
    pub async fn image_exists(&self, image: &str) -> Result<bool> {
        let _ = image;
        todo!("ferry-docker: image_exists")
    }

    /// Pull an image (a bare name means `:latest`), writing progress to `logs`
    /// (one line per layer status change, not every progress tick).
    pub async fn pull_image(&self, image: &str, logs: &LogSink) -> Result<()> {
        let _ = (image, logs);
        todo!("ferry-docker: pull_image")
    }

    /// Pull only if not present locally.
    pub async fn ensure_image(&self, image: &str, logs: &LogSink) -> Result<()> {
        let _ = (image, logs);
        todo!("ferry-docker: ensure_image")
    }

    /// TCP ports declared with EXPOSE in the image config, sorted ascending.
    pub async fn image_exposed_ports(&self, image: &str) -> Result<Vec<u16>> {
        let _ = image;
        todo!("ferry-docker: image_exposed_ports")
    }

    /// Remove an image (missing = Ok; in use = Err(Conflict)).
    pub async fn remove_image(&self, image: &str) -> Result<()> {
        let _ = image;
        todo!("ferry-docker: remove_image")
    }

    /// Create and start a container; returns its info after start. A name
    /// clash returns `Error::Conflict`; a missing image is NOT pulled
    /// automatically (call `ensure_image` first).
    pub async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerInfo> {
        let _ = spec;
        todo!("ferry-docker: run_container")
    }

    /// Start an existing (stopped) container.
    pub async fn start_container(&self, id: &str) -> Result<()> {
        let _ = id;
        todo!("ferry-docker: start_container")
    }

    /// Graceful stop (SIGTERM, then SIGKILL after `timeout_secs`). Missing or
    /// already-stopped = Ok.
    pub async fn stop_container(&self, id: &str, timeout_secs: u32) -> Result<()> {
        let _ = (id, timeout_secs);
        todo!("ferry-docker: stop_container")
    }

    /// Remove a container (and its anonymous volumes). Missing = Ok.
    pub async fn remove_container(&self, id: &str, force: bool) -> Result<()> {
        let _ = (id, force);
        todo!("ferry-docker: remove_container")
    }

    /// Inspect by id or name; `None` if it doesn't exist.
    pub async fn inspect_container(&self, id_or_name: &str) -> Result<Option<ContainerInfo>> {
        let _ = id_or_name;
        todo!("ferry-docker: inspect_container")
    }

    /// Containers having ALL the given labels (`key=value`), optionally
    /// including stopped ones. `host_port` must be populated for running ones.
    pub async fn list_containers(&self, labels: &[(&str, &str)], include_stopped: bool) -> Result<Vec<ContainerInfo>> {
        let _ = (labels, include_stopped);
        todo!("ferry-docker: list_containers")
    }

    /// Wait until the container exits; returns its exit code.
    pub async fn wait_container(&self, id: &str) -> Result<i64> {
        let _ = id;
        todo!("ferry-docker: wait_container")
    }

    /// Container output as a stream of lines (stdout/stderr kinds preserved,
    /// `ts` from Docker timestamps, `instance` unset). With `follow`, the
    /// stream ends when the container stops; errors end the stream.
    pub fn logs(&self, id: &str, follow: bool, tail: Option<usize>) -> LogStream {
        let _ = (id, follow, tail);
        todo!("ferry-docker: logs")
    }

    /// One-shot CPU / memory usage.
    pub async fn stats(&self, id: &str) -> Result<ContainerStats> {
        let _ = id;
        todo!("ferry-docker: stats")
    }

    /// Run a command inside a running container and collect its output.
    pub async fn exec(&self, id: &str, cmd: &[&str]) -> Result<ExecOutput> {
        let _ = (id, cmd);
        todo!("ferry-docker: exec")
    }
}

/// Find a free TCP port on 127.0.0.1 (bind to port 0 and release it).
pub fn free_host_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}
