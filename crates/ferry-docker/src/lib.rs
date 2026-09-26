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
//!
//! Error mapping: a daemon 404 becomes `Error::NotFound` (or `Ok` for the
//! idempotent stop/remove operations), a 409 becomes `Error::Conflict`, and
//! everything else `Error::Docker` with context ("starting container X: …").
//! Malformed container/volume/network/image references are rejected with
//! `Error::Invalid` before any request is made.

mod connect;
mod convert;
mod errors;
#[cfg(test)]
mod fake_daemon;
mod image_ref;
mod logs;
mod stats;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use bollard::errors::Error as BollardError;
use bollard::exec::{StartExecOptions, StartExecResults};
use bollard::models::{ExecConfig, NetworkCreateRequest, VolumeCreateRequest};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder, RemoveVolumeOptions, StatsOptionsBuilder,
    StopContainerOptionsBuilder, TagImageOptionsBuilder, WaitContainerOptionsBuilder,
};
use ferry_core::{Error, LogSink, LogStream, Result};
use futures::StreamExt;
use tracing::{debug, info, warn};

/// Timeout for calls that block until something happens in a container
/// (wait, exec). The daemon answers their headers immediately, so bollard's
/// request timeout would not normally apply, but a generous bound keeps a
/// custom short client timeout from breaking them.
const BLOCKING_CALL_TIMEOUT: Duration = Duration::from_secs(30 * 24 * 3600);
/// Extra time granted to `stop` beyond the container's grace period.
const STOP_TIMEOUT_MARGIN_SECS: u64 = 60;
/// Output kept by `exec` (the rest is drained and dropped).
const MAX_EXEC_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// How long `exec` waits for the daemon to report the exit code once the
/// output stream closed.
const EXEC_EXIT_POLL: Duration = Duration::from_millis(50);
const EXEC_EXIT_POLL_ATTEMPTS: u32 = 200;

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
    ///
    /// The API version starts at bollard's default and is lowered to the
    /// daemon's maximum when the daemon is older (or set to the version the
    /// daemon demands when it rejects ours). `DOCKER_HOST` supports `unix://`
    /// and plain `tcp://`/`http://` (no TLS, no ssh).
    pub async fn connect() -> Result<Self> {
        let docker_host = std::env::var("DOCKER_HOST").ok();
        let home = std::env::var("HOME").ok();
        let mut failures = Vec::new();
        for endpoint in connect::candidates(docker_host.as_deref(), home.as_deref())? {
            match connect::connect_endpoint(&endpoint).await {
                Ok(inner) => {
                    info!(%endpoint, api_version = %inner.client_version(), "connected to Docker");
                    return Ok(Docker { inner });
                }
                Err(err) => {
                    debug!(%endpoint, error = %err, "Docker endpoint unavailable");
                    failures.push(format!("{endpoint}: {err}"));
                }
            }
        }
        Err(Error::Docker(format!("cannot connect to the Docker daemon ({})", failures.join("; "))))
    }

    /// Wrap an existing bollard client.
    pub fn from_bollard(inner: bollard::Docker) -> Self {
        Docker { inner }
    }

    /// Access the underlying bollard client (escape hatch).
    pub fn bollard(&self) -> &bollard::Docker {
        &self.inner
    }

    /// A client for calls that block until something happens in a container.
    fn blocking_client(&self) -> bollard::Docker {
        self.inner.clone().with_timeout(BLOCKING_CALL_TIMEOUT.max(self.inner.timeout()))
    }

    /// Docker server version string, e.g. `29.2.0`.
    pub async fn version(&self) -> Result<String> {
        let version = self.inner.version().await.map_err(|e| errors::map_docker(e, "querying the Docker version"))?;
        version
            .version
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Error::Docker("querying the Docker version: the daemon did not report one".into()))
    }

    /// Create a bridge network if it doesn't exist.
    pub async fn ensure_network(&self, name: &str, labels: &BTreeMap<String, String>) -> Result<()> {
        errors::check_object_ref("network", name)?;
        match self.inner.inspect_network(name, None).await {
            // The lookup also matches id prefixes: only an exact name counts.
            Ok(network) if network.name.as_deref() == Some(name) => return Ok(()),
            Ok(_) => {}
            Err(e) if errors::is_not_found(&e) => {}
            Err(e) => return Err(errors::map_docker(e, &format!("inspecting network {name}"))),
        }
        let request = NetworkCreateRequest {
            name: name.to_string(),
            driver: Some("bridge".to_string()),
            labels: convert::labels_to_api(labels),
            ..Default::default()
        };
        match self.inner.create_network(request).await {
            Ok(_) => {
                info!(network = name, "created Docker network");
                Ok(())
            }
            // Created concurrently by someone else.
            Err(e) if errors::is_conflict(&e) => Ok(()),
            Err(e) => Err(errors::map_docker(e, &format!("creating network {name}"))),
        }
    }

    /// Create a named volume if it doesn't exist.
    pub async fn ensure_volume(&self, name: &str, labels: &BTreeMap<String, String>) -> Result<()> {
        errors::check_object_ref("volume", name)?;
        match self.inner.inspect_volume(name).await {
            Ok(_) => return Ok(()),
            Err(e) if errors::is_not_found(&e) => {}
            Err(e) => return Err(errors::map_docker(e, &format!("inspecting volume {name}"))),
        }
        let request = VolumeCreateRequest {
            name: Some(name.to_string()),
            labels: convert::labels_to_api(labels),
            ..Default::default()
        };
        match self.inner.create_volume(request).await {
            Ok(_) => {
                info!(volume = name, "created Docker volume");
                Ok(())
            }
            Err(e) if errors::is_conflict(&e) => Ok(()),
            Err(e) => Err(errors::map_docker(e, &format!("creating volume {name}"))),
        }
    }

    /// Remove a named volume (missing = Ok).
    pub async fn remove_volume(&self, name: &str) -> Result<()> {
        errors::check_object_ref("volume", name)?;
        match self.inner.remove_volume(name, None::<RemoveVolumeOptions>).await {
            Ok(()) => {
                info!(volume = name, "removed Docker volume");
                Ok(())
            }
            Err(e) if errors::is_not_found(&e) => Ok(()),
            Err(e) => Err(errors::map_docker(e, &format!("removing volume {name}"))),
        }
    }

    /// True if a named volume exists.
    pub async fn volume_exists(&self, name: &str) -> Result<bool> {
        errors::check_object_ref("volume", name)?;
        match self.inner.inspect_volume(name).await {
            Ok(_) => Ok(true),
            Err(e) if errors::is_not_found(&e) => Ok(false),
            Err(e) => Err(errors::map_docker(e, &format!("inspecting volume {name}"))),
        }
    }

    /// Add the reference `target` (`repo[:tag]`, tag defaults to `latest`;
    /// digests are not allowed) to the local image `source`, e.g. to pin a
    /// pulled `nginx:alpine` as `ferry/web:dep-…`. A missing source is
    /// `NotFound`; an existing `target` is moved to the new image.
    pub async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        errors::check_image_ref(source)?;
        errors::check_image_ref(target)?;
        if target.contains('@') {
            return Err(Error::invalid(format!("cannot tag with a digest reference '{target}'")));
        }
        let (repo, tag) = image_ref::split_image_ref(target)?;
        let opts = TagImageOptionsBuilder::new().repo(&repo).tag(&tag).build();
        self.inner
            .tag_image(source, Some(opts))
            .await
            // The only 404 here is a missing source image.
            .map_err(|e| errors::map(e, &format!("tagging image {source} as {target}"), "image", source))?;
        debug!(source, target, "tagged image");
        Ok(())
    }

    /// True if the image exists locally.
    pub async fn image_exists(&self, image: &str) -> Result<bool> {
        errors::check_image_ref(image)?;
        match self.inner.inspect_image(image).await {
            Ok(_) => Ok(true),
            Err(e) if errors::is_not_found(&e) => Ok(false),
            Err(e) => Err(errors::map_docker(e, &format!("inspecting image {image}"))),
        }
    }

    /// Pull an image (a bare name means `:latest`), writing progress to `logs`
    /// (one line per layer status change, not every progress tick).
    ///
    /// Progress lines are `stdout` lines (`"<layer>: <status>"`, or the
    /// daemon's own messages such as `Digest: …`); a final system line
    /// `==> Pulled <image>` is written on success. An image the registry does
    /// not know (or refuses) is `Error::NotFound`.
    pub async fn pull_image(&self, image: &str, logs: &LogSink) -> Result<()> {
        errors::check_image_ref(image)?;
        let (from_image, tag) = image_ref::split_image_ref(image)?;
        let options = CreateImageOptionsBuilder::new().from_image(&from_image).tag(&tag).build();
        info!(image, "pulling image");
        let mut progress = self.inner.create_image(Some(options), None, None);
        let mut layers: HashMap<String, String> = HashMap::new();
        while let Some(item) = progress.next().await {
            match item {
                Ok(update) => {
                    if let Some(line) = pull_progress_line(&mut layers, update.id.as_deref(), update.status.as_deref())
                    {
                        logs.stdout(line);
                    }
                }
                Err(e) if errors::is_not_found(&e) => {
                    debug!(image, error = %errors::detail(&e), "image not found by the registry");
                    return Err(Error::not_found("image", image));
                }
                Err(e) => return Err(Error::Docker(format!("pulling image {image}: {}", errors::detail(&e)))),
            }
        }
        info!(image, "pulled image");
        logs.system(format!("==> Pulled {image}"));
        Ok(())
    }

    /// Pull only if not present locally.
    pub async fn ensure_image(&self, image: &str, logs: &LogSink) -> Result<()> {
        if self.image_exists(image).await? {
            debug!(image, "image present locally");
            return Ok(());
        }
        self.pull_image(image, logs).await
    }

    /// TCP ports declared with EXPOSE in the image config, sorted ascending.
    /// A missing image is `Error::NotFound`.
    pub async fn image_exposed_ports(&self, image: &str) -> Result<Vec<u16>> {
        errors::check_image_ref(image)?;
        let inspect = self
            .inner
            .inspect_image(image)
            .await
            .map_err(|e| errors::map(e, &format!("inspecting image {image}"), "image", image))?;
        let keys = inspect.config.and_then(|c| c.exposed_ports).unwrap_or_default();
        Ok(convert::exposed_tcp_ports(&keys))
    }

    /// Remove an image (missing = Ok; in use = Err(Conflict)).
    pub async fn remove_image(&self, image: &str) -> Result<()> {
        errors::check_image_ref(image)?;
        let options = RemoveImageOptionsBuilder::new().force(false).noprune(false).build();
        match self.inner.remove_image(image, Some(options), None).await {
            Ok(_) => {
                info!(image, "removed image");
                Ok(())
            }
            Err(e) if errors::is_not_found(&e) => Ok(()),
            Err(e) => Err(errors::map_docker(e, &format!("removing image {image}"))),
        }
    }

    /// Create and start a container; returns its info after start. A name
    /// clash returns `Error::Conflict`; a missing image is NOT pulled
    /// automatically (call `ensure_image` first).
    ///
    /// A missing image (or network) is `Error::NotFound`. If starting fails
    /// the created container is removed again, so an error leaves nothing
    /// behind.
    pub async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerInfo> {
        if !spec.name.is_empty() {
            errors::check_object_ref("container", &spec.name)?;
        }
        if spec.image.trim().is_empty() {
            return Err(Error::invalid("container image must not be empty"));
        }
        let label = if spec.name.is_empty() { spec.image.as_str() } else { spec.name.as_str() };
        if spec.network.as_deref().is_none_or(str::is_empty) && !spec.network_aliases.is_empty() {
            warn!(container = label, "network aliases ignored: the container joins no user-defined network");
        }
        let mut options = CreateContainerOptionsBuilder::new();
        if !spec.name.is_empty() {
            options = options.name(&spec.name);
        }
        let created = match self.inner.create_container(Some(options.build()), convert::create_body(spec)).await {
            Ok(created) => created,
            Err(e) => return Err(create_error(e, spec, label)),
        };
        for warning in created.warnings.iter().filter(|w| !w.is_empty()) {
            warn!(container = label, warning = %warning, "Docker warning on container create");
        }
        let id = created.id;

        if let Err(e) = self.inner.start_container(&id, None).await {
            let err = errors::map(e, &format!("starting container {label}"), "container", label);
            self.discard_container(&id, label).await;
            return Err(err);
        }
        match self.inspect_container(&id).await {
            Ok(Some(info)) => {
                info!(container = %info.name, id = %short_id(&info.id), image = %spec.image, host_port = ?info.host_port, "started container");
                Ok(info)
            }
            Ok(None) => Err(Error::Docker(format!("starting container {label}: it was removed right after starting"))),
            Err(err) => {
                self.discard_container(&id, label).await;
                Err(err)
            }
        }
    }

    /// Best-effort cleanup of a container that failed to come up.
    async fn discard_container(&self, id: &str, label: &str) {
        if let Err(e) = self.remove_container(id, true).await {
            warn!(container = label, error = %e, "could not remove container after failed start");
        }
    }

    /// Start an existing (stopped) container.
    pub async fn start_container(&self, id: &str) -> Result<()> {
        errors::check_object_ref("container", id)?;
        // 304 (already running) is a success for bollard.
        self.inner
            .start_container(id, None)
            .await
            .map_err(|e| errors::map(e, &format!("starting container {id}"), "container", id))
    }

    /// Graceful stop (SIGTERM, then SIGKILL after `timeout_secs`). Missing or
    /// already-stopped = Ok.
    pub async fn stop_container(&self, id: &str, timeout_secs: u32) -> Result<()> {
        errors::check_object_ref("container", id)?;
        let grace = i32::try_from(timeout_secs).unwrap_or(i32::MAX);
        let options = StopContainerOptionsBuilder::new().t(grace).build();
        // The request lasts up to the grace period: give it room.
        let request_timeout =
            Duration::from_secs(u64::from(timeout_secs) + STOP_TIMEOUT_MARGIN_SECS).max(self.inner.timeout());
        let client = self.inner.clone().with_timeout(request_timeout);
        match client.stop_container(id, Some(options)).await {
            Ok(()) => {
                debug!(container = id, "stopped container");
                Ok(())
            }
            Err(e) if errors::is_not_found(&e) => Ok(()),
            Err(e) => Err(errors::map_docker(e, &format!("stopping container {id}"))),
        }
    }

    /// Remove a container (and its anonymous volumes). Missing = Ok.
    ///
    /// Without `force`, removing a running container is `Error::Conflict`.
    /// A removal already in progress counts as success.
    pub async fn remove_container(&self, id: &str, force: bool) -> Result<()> {
        errors::check_object_ref("container", id)?;
        let options = RemoveContainerOptionsBuilder::new().force(force).v(true).build();
        match self.inner.remove_container(id, Some(options)).await {
            Ok(()) => {
                debug!(container = id, "removed container");
                Ok(())
            }
            Err(e) if errors::is_not_found(&e) => Ok(()),
            Err(e) if errors::is_conflict(&e) && errors::detail(&e).contains("already in progress") => Ok(()),
            Err(e) => Err(errors::map_docker(e, &format!("removing container {id}"))),
        }
    }

    /// Inspect by id or name; `None` if it doesn't exist.
    pub async fn inspect_container(&self, id_or_name: &str) -> Result<Option<ContainerInfo>> {
        errors::check_object_ref("container", id_or_name)?;
        match self.inner.inspect_container(id_or_name, None).await {
            Ok(resp) => Ok(Some(convert::info_from_inspect(resp))),
            Err(e) if errors::is_not_found(&e) => Ok(None),
            Err(e) => Err(errors::map_docker(e, &format!("inspecting container {id_or_name}"))),
        }
    }

    /// Containers having ALL the given labels (`key=value`), optionally
    /// including stopped ones. `host_port` must be populated for running ones.
    ///
    /// The list API does not report start times or restart counts:
    /// `started_at` and `restart_count` are `None` here (use
    /// [`Docker::inspect_container`] for them). `exit_code` is parsed from
    /// Docker's status text for exited containers.
    pub async fn list_containers(&self, labels: &[(&str, &str)], include_stopped: bool) -> Result<Vec<ContainerInfo>> {
        let mut options = ListContainersOptionsBuilder::new().all(include_stopped);
        if !labels.is_empty() {
            let filters: HashMap<&str, Vec<String>> =
                HashMap::from([("label", labels.iter().map(|(k, v)| format!("{k}={v}")).collect())]);
            options = options.filters(&filters);
        }
        let containers = self
            .inner
            .list_containers(Some(options.build()))
            .await
            .map_err(|e| errors::map_docker(e, "listing containers"))?;
        Ok(containers.into_iter().map(convert::info_from_summary).collect())
    }

    /// Wait until the container exits; returns its exit code.
    ///
    /// Returns immediately for a container that is not running (its last exit
    /// code, 0 if it never ran). A missing container is `Error::NotFound`.
    pub async fn wait_container(&self, id: &str) -> Result<i64> {
        errors::check_object_ref("container", id)?;
        let options = WaitContainerOptionsBuilder::new().condition("not-running").build();
        let client = self.blocking_client();
        let mut responses = client.wait_container(id, Some(options));
        match responses.next().await {
            Some(Ok(resp)) => match resp.error.and_then(|e| e.message).filter(|m| !m.is_empty()) {
                Some(message) => Err(Error::Docker(format!("waiting for container {id}: {message}"))),
                None => Ok(resp.status_code),
            },
            // bollard reports non-zero exit codes as errors.
            Some(Err(BollardError::DockerContainerWaitError { code, error })) => {
                if !error.is_empty() {
                    debug!(container = id, code, error = %error, "container wait reported an error");
                }
                Ok(code)
            }
            Some(Err(e)) => Err(errors::map(e, &format!("waiting for container {id}"), "container", id)),
            None => Err(Error::Docker(format!("waiting for container {id}: the daemon sent no exit status"))),
        }
    }

    /// Container output as a stream of lines (stdout/stderr kinds preserved,
    /// `ts` from Docker timestamps, `instance` unset). With `follow`, the
    /// stream ends when the container stops; errors end the stream.
    ///
    /// `tail: None` means all available lines. A missing container yields an
    /// empty stream.
    pub fn logs(&self, id: &str, follow: bool, tail: Option<usize>) -> LogStream {
        if let Err(e) = errors::check_object_ref("container", id) {
            debug!(error = %e, "not streaming logs");
            return Box::pin(futures::stream::empty());
        }
        let client = if follow { self.blocking_client() } else { self.inner.clone() };
        logs::stream(client, id.to_string(), follow, tail)
    }

    /// One-shot CPU / memory usage.
    ///
    /// Takes about one second (the daemon samples CPU twice). CPU is in
    /// percent of one core, like `docker stats`; memory excludes the page
    /// cache. A stopped container reports zeros; a missing one is
    /// `Error::NotFound`.
    pub async fn stats(&self, id: &str) -> Result<ContainerStats> {
        errors::check_object_ref("container", id)?;
        let options = StatsOptionsBuilder::new().stream(false).one_shot(false).build();
        let mut samples = self.inner.stats(id, Some(options));
        match samples.next().await {
            Some(Ok(resp)) => Ok(stats::from_response(&resp)),
            Some(Err(e)) => Err(errors::map(e, &format!("reading stats of container {id}"), "container", id)),
            None => Err(Error::Docker(format!("reading stats of container {id}: the daemon sent no sample"))),
        }
    }

    /// Run a command inside a running container and collect its output.
    ///
    /// Waits for the command to finish (no timeout: wrap the call in
    /// `tokio::time::timeout` for probes). A missing container is
    /// `Error::NotFound`, a stopped one `Error::Conflict`. Output beyond 8 MiB
    /// is dropped.
    ///
    /// Error messages name only the program (`cmd[0]`), never its arguments.
    /// Still, do not put secrets in `cmd` — anyone who can inspect the exec
    /// or list the container's processes sees them: pass them with
    /// [`Docker::exec_with_env`] instead (e.g. `REDISCLI_AUTH` for `redis-cli`).
    pub async fn exec(&self, id: &str, cmd: &[&str]) -> Result<ExecOutput> {
        self.exec_with_env(id, cmd, &[]).await
    }

    /// [`Docker::exec`] with extra environment variables for the command (on
    /// top of the container's own). This is how to hand a command a
    /// credential: values never appear in error messages or logs.
    ///
    /// Names must be non-empty and contain neither `=` nor NUL; values must
    /// not contain NUL (`Error::Invalid`, naming the variable, not its value).
    pub async fn exec_with_env(&self, id: &str, cmd: &[&str], env: &[(&str, &str)]) -> Result<ExecOutput> {
        errors::check_object_ref("container", id)?;
        let Some(program) = cmd.first().filter(|p| !p.trim().is_empty()) else {
            return Err(Error::invalid("exec: command must not be empty"));
        };
        let env = exec_env(env)?;
        let context = || exec_context(id, program);
        let config = ExecConfig {
            attach_stdin: Some(false),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(false),
            cmd: Some(cmd.iter().map(|s| (*s).to_string()).collect()),
            env,
            ..Default::default()
        };
        let created =
            self.inner.create_exec(id, config).await.map_err(|e| errors::map(e, &context(), "container", id))?;

        let client = self.blocking_client();
        let started = client
            .start_exec(&created.id, Some(StartExecOptions { detach: false, tty: false, output_capacity: None }))
            .await
            .map_err(|e| errors::map_docker(e, &context()))?;
        let StartExecResults::Attached { mut output, input } = started else {
            return Err(Error::Docker(format!("{}: the daemon did not attach the output", context())));
        };
        let mut collected: Vec<u8> = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = output.next().await {
            let chunk = chunk.map_err(|e| errors::map_docker(e, &context()))?;
            let bytes = chunk.into_bytes();
            let room = MAX_EXEC_OUTPUT_BYTES.saturating_sub(collected.len());
            if bytes.len() > room {
                truncated = true;
            }
            collected.extend_from_slice(&bytes[..bytes.len().min(room)]);
        }
        drop(input);
        if truncated {
            debug!(container = id, "exec output truncated to {MAX_EXEC_OUTPUT_BYTES} bytes");
        }

        // The output closes when the process exits; the exit code can lag a moment.
        for _ in 0..EXEC_EXIT_POLL_ATTEMPTS {
            let inspect = self.inner.inspect_exec(&created.id).await.map_err(|e| errors::map_docker(e, &context()))?;
            if inspect.running != Some(true)
                && let Some(exit_code) = inspect.exit_code
            {
                return Ok(ExecOutput { exit_code, output: String::from_utf8_lossy(&collected).into_owned() });
            }
            tokio::time::sleep(EXEC_EXIT_POLL).await;
        }
        Err(Error::Docker(format!("{}: the daemon did not report an exit code", context())))
    }
}

/// Error context of an exec. It names the program only: arguments often
/// carry credentials (`redis-cli -a <password>`), and error messages end up
/// in logs, the store and API responses.
fn exec_context(id: &str, program: &str) -> String {
    format!("running {program:?} in container {id}")
}

/// `KEY=value` entries for an exec (`None` when there are none). Errors name
/// the variable, never its value.
fn exec_env(env: &[(&str, &str)]) -> Result<Option<Vec<String>>> {
    if env.is_empty() {
        return Ok(None);
    }
    env.iter()
        .map(|(key, value)| {
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(Error::invalid(format!("exec: invalid environment variable name {key:?}")));
            }
            if value.contains('\0') {
                return Err(Error::invalid(format!(
                    "exec: the value of environment variable {key} contains a NUL byte"
                )));
            }
            Ok(format!("{key}={value}"))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// Map a container-create error. A 404 is about the image or the network.
fn create_error(err: BollardError, spec: &ContainerSpec, label: &str) -> Error {
    // Unless the daemon names something else, a 404 here is the image.
    errors::map(err, &format!("creating container {label}"), "image", &spec.image)
}

/// One progress line per layer status change: `"<id>: <status>"` for layer
/// updates, the bare status for general messages. Repeated statuses (the
/// progress ticks of "Downloading", "Extracting") yield nothing.
fn pull_progress_line(layers: &mut HashMap<String, String>, id: Option<&str>, status: Option<&str>) -> Option<String> {
    let status = status.map(str::trim).filter(|s| !s.is_empty())?;
    match id.map(str::trim).filter(|i| !i.is_empty()) {
        Some(id) => {
            if layers.get(id).is_some_and(|last| last == status) {
                return None;
            }
            layers.insert(id.to_string(), status.to_string());
            Some(format!("{id}: {status}"))
        }
        None => Some(status.to_string()),
    }
}

fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// Find a free TCP port on 127.0.0.1 (bind to port 0 and release it).
pub fn free_host_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_progress_dedupes_layer_statuses() {
        let mut layers = HashMap::new();
        let mut lines = Vec::new();
        let updates = [
            (Some("stable"), Some("Pulling from library/busybox")),
            (Some("a1b2"), Some("Pulling fs layer")),
            (Some("a1b2"), Some("Downloading")),
            (Some("a1b2"), Some("Downloading")),
            (Some("a1b2"), Some("Downloading")),
            (Some("c3d4"), Some("Waiting")),
            (Some("a1b2"), Some("Download complete")),
            (Some("a1b2"), Some("Extracting")),
            (Some("a1b2"), Some("Extracting")),
            (Some("a1b2"), Some("Pull complete")),
            (None, Some("Digest: sha256:abc")),
            (Some(""), Some("Status: Downloaded newer image for busybox:stable")),
            (Some("a1b2"), None),
            (None, Some("  ")),
        ];
        for (id, status) in updates {
            if let Some(line) = pull_progress_line(&mut layers, id, status) {
                lines.push(line);
            }
        }
        assert_eq!(
            lines,
            [
                "stable: Pulling from library/busybox",
                "a1b2: Pulling fs layer",
                "a1b2: Downloading",
                "c3d4: Waiting",
                "a1b2: Download complete",
                "a1b2: Extracting",
                "a1b2: Pull complete",
                "Digest: sha256:abc",
                "Status: Downloaded newer image for busybox:stable",
            ]
        );
    }

    #[test]
    fn create_errors() {
        let not_found = |m: &str| BollardError::DockerResponseServerError { status_code: 404, message: m.into() };
        let spec = ContainerSpec { image: "nope:1".into(), network: Some("ferrynet".into()), ..Default::default() };
        let e = create_error(not_found("No such image: nope:1"), &spec, "c");
        assert!(matches!(&e, Error::NotFound(m) if m == "image 'nope:1'"), "{e:?}");
        let e = create_error(not_found("network ferrynet not found"), &spec, "c");
        assert!(matches!(&e, Error::NotFound(m) if m == "network 'ferrynet'"), "{e:?}");
        let conflict = BollardError::DockerResponseServerError { status_code: 409, message: "name in use".into() };
        assert!(matches!(create_error(conflict, &spec, "c"), Error::Conflict(_)));
        let bad = BollardError::DockerResponseServerError { status_code: 400, message: "invalid reference".into() };
        assert!(
            matches!(create_error(bad, &spec, "c"), Error::Docker(m) if m == "creating container c: invalid reference")
        );
    }

    #[test]
    fn container_state_parsing() {
        for s in ["created", "running", "paused", "restarting", "removing", "exited", "dead"] {
            assert_eq!(ContainerState::parse(s).as_str(), s);
        }
        assert_eq!(ContainerState::parse("RUNNING"), ContainerState::Running);
        assert_eq!(ContainerState::parse("stopping"), ContainerState::Unknown);
        assert!(ContainerState::Running.is_running());
        assert!(!ContainerState::Restarting.is_running());
    }

    #[test]
    fn free_port() {
        let port = free_host_port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn short_ids() {
        assert_eq!(short_id("0123456789abcdef"), "0123456789ab");
        assert_eq!(short_id("abc"), "abc");
    }

    use crate::fake_daemon::{FakeDaemon, Request};

    fn api_error(status: u16, message: &str) -> (u16, String) {
        (status, serde_json::json!({ "message": message }).to_string())
    }

    fn error_text(err: &Error) -> String {
        match err {
            Error::Docker(m) | Error::Conflict(m) | Error::NotFound(m) | Error::Invalid(m) => m.clone(),
            other => other.to_string(),
        }
    }

    #[test]
    fn exec_context_names_the_program_only() {
        assert_eq!(exec_context("c1", "redis-cli"), r#"running "redis-cli" in container c1"#);
    }

    #[test]
    fn exec_env_entries() {
        assert_eq!(exec_env(&[]).unwrap(), None);
        assert_eq!(
            exec_env(&[("REDISCLI_AUTH", "p=w"), ("EMPTY", "")]).unwrap(),
            Some(vec!["REDISCLI_AUTH=p=w".to_string(), "EMPTY=".to_string()])
        );
        for key in ["", "A=B", "A\0B"] {
            assert!(matches!(exec_env(&[(key, "v")]), Err(Error::Invalid(_))), "{key:?}");
        }
        let err = exec_env(&[("TOKEN", "s3cret\0x")]).unwrap_err();
        assert!(matches!(&err, Error::Invalid(m) if m.contains("TOKEN") && !m.contains("s3cret")), "{err:?}");
    }

    /// Regression: exec errors used to embed the whole argv ("running
    /// \"redis-cli -a <password> ping\" …"), leaking credentials into logs,
    /// datastore errors and API responses.
    #[tokio::test]
    async fn exec_errors_do_not_reveal_arguments() {
        let daemon = FakeDaemon::start(|req: &Request| match req.target.as_str() {
            "/containers/broken/exec" => api_error(500, "boom"),
            "/containers/stopped/exec" => api_error(409, "container stopped is not running"),
            _ => api_error(404, "No such container: gone"),
        })
        .await;
        let argv = ["redis-cli", "--no-auth-warning", "-a", "s3cret-password", "ping"];

        let err = daemon.docker.exec("broken", &argv).await.unwrap_err();
        assert!(matches!(&err, Error::Docker(m) if m == r#"running "redis-cli" in container broken: boom"#), "{err:?}");
        let err = daemon.docker.exec("stopped", &argv).await.unwrap_err();
        assert!(
            matches!(&err, Error::Conflict(m)
                if m == r#"running "redis-cli" in container stopped: container stopped is not running"#),
            "{err:?}"
        );
        let err = daemon.docker.exec("gone", &argv).await.unwrap_err();
        assert!(matches!(&err, Error::NotFound(m) if m == "container 'gone'"), "{err:?}");

        let requests = daemon.requests();
        assert_eq!(requests.len(), 3);
        let body = requests[0].json();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(body["Cmd"], serde_json::json!(argv), "the command itself is passed unchanged");
        assert!(body.get("Env").is_none(), "no env unless asked: {body}");
    }

    #[tokio::test]
    async fn exec_with_env_passes_secrets_outside_the_command() {
        let daemon = FakeDaemon::start(|_: &Request| api_error(500, "boom")).await;
        let err = daemon
            .docker
            .exec_with_env("c1", &["redis-cli", "ping"], &[("REDISCLI_AUTH", "s3cret-password")])
            .await
            .unwrap_err();
        let text = error_text(&err);
        assert_eq!(text, r#"running "redis-cli" in container c1: boom"#);
        assert!(!err.to_string().contains("s3cret"), "{err}");

        let requests = daemon.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].target, "/containers/c1/exec");
        let body = requests[0].json();
        assert_eq!(body["Cmd"], serde_json::json!(["redis-cli", "ping"]));
        assert_eq!(body["Env"], serde_json::json!(["REDISCLI_AUTH=s3cret-password"]));

        // Invalid input never reaches the daemon, and errors don't echo values.
        let err = daemon.docker.exec_with_env("c1", &["true"], &[("BAD=KEY", "s3cret")]).await.unwrap_err();
        assert!(matches!(&err, Error::Invalid(m) if !m.contains("s3cret")), "{err:?}");
        assert!(matches!(daemon.docker.exec("c1", &[]).await, Err(Error::Invalid(_))));
        assert!(matches!(daemon.docker.exec("c1", &[" "]).await, Err(Error::Invalid(_))));
        assert!(matches!(daemon.docker.exec("../c1", &["true"]).await, Err(Error::Invalid(_))));
        assert_eq!(daemon.requests().len(), 1);
    }

    #[tokio::test]
    async fn volume_exists_maps_daemon_answers() {
        let daemon = FakeDaemon::start(|req: &Request| match req.target.as_str() {
            "/volumes/present" => (
                200,
                serde_json::json!({
                    "Name": "present", "Driver": "local", "Mountpoint": "/var/lib/docker/volumes/present/_data",
                    "Labels": {}, "Scope": "local", "Options": {}
                })
                .to_string(),
            ),
            "/volumes/broken" => api_error(500, "boom"),
            _ => api_error(404, "get missing: no such volume"),
        })
        .await;
        assert!(daemon.docker.volume_exists("present").await.unwrap());
        assert!(!daemon.docker.volume_exists("missing").await.unwrap());
        let err = daemon.docker.volume_exists("broken").await.unwrap_err();
        assert!(matches!(&err, Error::Docker(m) if m == "inspecting volume broken: boom"), "{err:?}");
        assert!(matches!(daemon.docker.volume_exists("../etc").await, Err(Error::Invalid(_))));
        assert!(matches!(daemon.docker.volume_exists("").await, Err(Error::Invalid(_))));

        let requests = daemon.requests();
        assert_eq!(requests.iter().map(|r| r.method.as_str()).collect::<Vec<_>>(), ["GET", "GET", "GET"]);
        assert_eq!(requests.len(), 3, "invalid names never reach the daemon");
    }

    #[tokio::test]
    async fn volume_exists_reports_an_unreachable_daemon() {
        // Nothing listens on a port we just released: an error, not "absent".
        let port = free_host_port().unwrap();
        let inner =
            bollard::Docker::connect_with_http(&format!("http://127.0.0.1:{port}"), 5, bollard::API_DEFAULT_VERSION)
                .unwrap();
        let err = Docker::from_bollard(inner).volume_exists("data").await.unwrap_err();
        assert!(matches!(&err, Error::Docker(m) if m.starts_with("inspecting volume data: ")), "{err:?}");
    }

    #[tokio::test]
    async fn tag_image_requests_and_errors() {
        let daemon = FakeDaemon::start(|req: &Request| {
            if req.target.starts_with("/images/nope:1/tag") {
                api_error(404, "No such image: nope:1")
            } else if req.target.starts_with("/images/busybox:stable/tag") {
                (201, String::new())
            } else {
                api_error(500, "unexpected request")
            }
        })
        .await;
        let docker = &daemon.docker;
        docker.tag_image("busybox:stable", "ferryfix-docker/x:1").await.unwrap();
        docker.tag_image("busybox:stable", "localhost:5000/app").await.unwrap();
        let err = docker.tag_image("nope:1", "ferryfix-docker/x:1").await.unwrap_err();
        assert!(matches!(&err, Error::NotFound(m) if m == "image 'nope:1'"), "{err:?}");

        let requests = daemon.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|r| r.method == "POST"));
        let query = |r: &Request| {
            let mut pairs: Vec<String> =
                r.target.split_once('?').map(|(_, q)| q).unwrap_or("").split('&').map(String::from).collect();
            pairs.sort();
            pairs
        };
        assert_eq!(query(&requests[0]), ["repo=ferryfix-docker%2Fx", "tag=1"]);
        assert_eq!(query(&requests[1]), ["repo=localhost%3A5000%2Fapp", "tag=latest"], "registry port is not a tag");

        // Rejected before any request.
        let digest = "sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662";
        for (source, target) in [
            ("busybox:stable", format!("ferryfix-docker/x@{digest}")),
            ("busybox:stable", "../x".to_string()),
            ("busybox:stable", "x:".to_string()),
            ("../busybox", "ferryfix-docker/x:1".to_string()),
            ("", "ferryfix-docker/x:1".to_string()),
        ] {
            assert!(matches!(docker.tag_image(source, &target).await, Err(Error::Invalid(_))), "{source} → {target}");
        }
        assert_eq!(daemon.requests().len(), 3);
    }
}
