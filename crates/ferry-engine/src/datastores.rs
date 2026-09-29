//! Managed Postgres / Redis: one container + one named volume each. Their
//! resource limits are set when the container is created and changed in
//! place (`docker update`, no restart) by [`update_limits`]; provisioning
//! also applies them to an existing container it keeps (see
//! [`sync_limits`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::naming::{LABEL_DATASTORE, LABEL_INSTANCE};
use ferry_core::{Datastore, DatastoreKind, DatastoreStatus, Error, LogSink, Naming, Result};
use ferry_docker::{
    ContainerInfo, ContainerSpec, ContainerState, HostInfo, PortPublish, RestartPolicy, VolumeMount, free_host_port,
};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::instances::{self, STOP_GRACE_SECS};
use crate::limits::{self, Resources};
use crate::state::{Inner, SetGuard, deleting_datastores_set};
use crate::util::{error_message, panic_message};

/// Minimum time a datastore gets to become ready (first start initializes
/// the data directory).
const MIN_READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_secs(1);
/// One readiness probe (exec) may take this long.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Where each kind keeps its data.
fn data_dir(kind: DatastoreKind) -> &'static str {
    match kind {
        DatastoreKind::Postgres => "/var/lib/postgresql/data",
        DatastoreKind::Redis => "/data",
    }
}

/// Redis's entrypoint, run before the image's own: sets `maxmemory` to 3/4
/// of the container's memory limit, read from its cgroup (v2, else v1) on
/// every start, so a limit changed in place still holds after a restart
/// (none when unlimited: cgroups report `max` or a 19-digit number). Over
/// `maxmemory` Redis refuses writes instead of being killed by the kernel
/// (and killed again replaying an append-only file that no longer fits);
/// the rest is headroom for the AOF rewrite's fork, client buffers and
/// fragmentation. Keep in sync with [`redis_maxmemory`].
pub(crate) const REDIS_START: &str = r#"limit=$(cat /sys/fs/cgroup/memory.max 2>/dev/null || cat /sys/fs/cgroup/memory/memory.limit_in_bytes 2>/dev/null)
case "$limit" in
''|*[!0-9]*) ;;
*) [ ${#limit} -le 15 ] && set -- "$@" --maxmemory $((limit / 4 * 3)) ;;
esac
exec docker-entrypoint.sh "$@""#;

/// Redis's `maxmemory` (bytes, 0 = none) for a container memory limit, as
/// [`REDIS_START`] computes it.
pub(crate) fn redis_maxmemory(memory_limit_bytes: Option<i64>) -> i64 {
    memory_limit_bytes.filter(|b| *b > 0).map_or(0, |b| b / 4 * 3)
}

/// Container spec of a datastore (`host_port` must be allocated).
pub(crate) fn datastore_spec(
    naming: &Naming,
    ds: &Datastore,
    bind_ip: &str,
    host_port: u16,
    resources: &Resources,
) -> ContainerSpec {
    let (env, cmd, entrypoint) = match ds.kind {
        DatastoreKind::Postgres => (
            vec![
                ("POSTGRES_USER".to_string(), ds.username.clone()),
                ("POSTGRES_PASSWORD".to_string(), ds.password.clone()),
                ("POSTGRES_DB".to_string(), ds.database.clone().unwrap_or_else(|| ds.username.clone())),
            ],
            None,
            None,
        ),
        DatastoreKind::Redis => (
            Vec::new(),
            Some(vec![
                "redis-server".to_string(),
                "--requirepass".to_string(),
                ds.password.clone(),
                "--appendonly".to_string(),
                "yes".to_string(),
            ]),
            Some(vec!["/bin/sh".to_string(), "-c".to_string(), REDIS_START.to_string(), "redis-start".to_string()]),
        ),
    };
    let mut spec = ContainerSpec {
        name: naming.datastore_container(&ds.name),
        image: ds.image(),
        env,
        cmd,
        entrypoint,
        labels: naming.datastore_labels(&ds.id),
        network: Some(naming.network()),
        network_aliases: vec![ds.name.clone()],
        publish: Some(PortPublish {
            container_port: ds.internal_port(),
            host_ip: bind_ip.to_string(),
            host_port: Some(host_port),
        }),
        volumes: vec![VolumeMount { volume: naming.datastore_volume(&ds.name), target: data_dir(ds.kind).to_string() }],
        restart_policy: RestartPolicy::UnlessStopped,
        memory_limit_bytes: None,
        nano_cpus: None,
        pids_limit: None,
        log_rotation: None,
        working_dir: None,
    };
    resources.apply(&mut spec);
    spec
}

/// The readiness command and the environment it runs with. Credentials go
/// in the environment, never in the argv (which Docker reports in exec
/// inspection and process listings).
pub(crate) fn ready_command(ds: &Datastore) -> (Vec<String>, Vec<(&'static str, String)>) {
    match ds.kind {
        // Over TCP: the entrypoint's temporary init server only listens on
        // the unix socket, so this is ready once the real server is up.
        DatastoreKind::Postgres => (
            vec![
                "pg_isready".into(),
                "-h".into(),
                "127.0.0.1".into(),
                "-p".into(),
                ds.internal_port().to_string(),
                "-U".into(),
                ds.username.clone(),
                "-d".into(),
                ds.database.clone().unwrap_or_else(|| ds.username.clone()),
            ],
            Vec::new(),
        ),
        // redis-cli reads the password from REDISCLI_AUTH.
        DatastoreKind::Redis => (vec!["redis-cli".into(), "ping".into()], vec![("REDISCLI_AUTH", ds.password.clone())]),
    }
}

/// `message` with every occurrence of the datastore's password masked
/// (defense in depth: container output and Docker errors should never
/// carry it, but they end up in the server log and the datastore's error).
pub(crate) fn redact(message: &str, ds: &Datastore) -> String {
    if ds.password.is_empty() { message.to_string() } else { message.replace(&ds.password, "********") }
}

fn is_ready(kind: DatastoreKind, exit_code: i64, output: &str) -> bool {
    match kind {
        DatastoreKind::Postgres => exit_code == 0,
        DatastoreKind::Redis => exit_code == 0 && output.contains("PONG"),
    }
}

/// `Engine::provision_datastore`: start provisioning in the background
/// (no-op when it is already being provisioned).
pub(crate) async fn provision(inner: &Arc<Inner>, datastore_id: &str) -> Result<()> {
    let ds = inner.store.require_datastore(datastore_id).await?;
    let token = inner.shutdown.child_token();
    let registered = inner.with_rt(|rt| {
        if rt.deleting_datastores.contains(&ds.id) {
            return Err(Error::conflict(format!("datastore '{}' is being deleted", ds.name)));
        }
        if rt.provisioning.contains_key(&ds.id) {
            return Ok(false);
        }
        rt.provisioning.insert(ds.id.clone(), token.clone());
        Ok(true)
    })?;
    if !registered {
        return Ok(());
    }
    let task_inner = inner.clone();
    inner.spawn(async move {
        let inner = task_inner;
        let id = ds.id.clone();
        let task = {
            let inner = inner.clone();
            let id = id.clone();
            inner.clone().spawn(async move { provision_task(&inner, &id, &token).await })
        };
        let outcome = match task.await {
            Ok(r) => r,
            Err(e) => Err(Error::internal(panic_message(&e))),
        };
        if let Err(e) = outcome {
            if matches!(e, Error::Canceled) {
                debug!(datastore = %ds.name, "provisioning canceled");
            } else {
                // Never let the password reach the server log or the API.
                let message = redact(&error_message(&e), &ds);
                error!(datastore = %ds.name, "provisioning failed: {message}");
                mark_failed(&inner, &id, &message).await;
            }
        }
        inner.with_rt(|rt| rt.provisioning.remove(&id));
    });
    Ok(())
}

async fn mark_failed(inner: &Inner, id: &str, message: &str) {
    match inner.store.find_datastore(id).await.ok().flatten() {
        Some(mut ds) => {
            ds.status = DatastoreStatus::Failed;
            ds.error = Some(message.to_string());
            if let Err(e) = inner.store.update_datastore(&ds).await {
                warn!(datastore = %ds.name, "cannot record provisioning failure: {e}");
            }
        }
        None => debug!(datastore = id, "datastore vanished while provisioning"),
    }
}

async fn provision_task(inner: &Arc<Inner>, id: &str, cancel: &CancellationToken) -> Result<()> {
    let _lock = tokio::select! {
        g = inner.datastore_locks.lock(id) => g,
        _ = cancel.cancelled() => return Err(Error::Canceled),
    };
    let mut ds = inner.store.require_datastore(id).await?;
    // The external port is allocated once and stored before the container starts.
    if ds.host_port.is_none() {
        ds.host_port = Some(free_host_port()?);
        ds = inner.store.update_datastore(&ds).await?;
    }
    let naming = &inner.naming;
    let name = naming.datastore_container(&ds.name);
    let existing = inner.docker.inspect_container(&name).await?;
    let running = existing.as_ref().is_some_and(|c| c.state.is_running() || c.state == ContainerState::Restarting);
    // Only show "creating" when there is something to (re)create or retry.
    if (!running && ds.status != DatastoreStatus::Creating)
        || ds.status == DatastoreStatus::Failed
        || ds.error.is_some()
    {
        ds.status = DatastoreStatus::Creating;
        ds.error = None;
        ds = inner.store.update_datastore(&ds).await?;
    }
    info!(datastore = %ds.name, kind = %ds.kind, image = %ds.image(), "provisioning datastore");

    let new_volume = check_volume_owner(inner, &ds).await? == VolumeOwner::Missing;
    // A new datastore (no volume yet): not on a full disk. Checked before the
    // volume is created, so a retry still counts as new.
    if new_volume {
        limits::check_free_disk(inner).await?;
    }
    inner.docker.ensure_volume(&naming.datastore_volume(&ds.name), &naming.datastore_labels(&ds.id)).await?;
    // An existing stopped container is started; one that can't be (its host
    // port was taken meanwhile, it is paused, dead...) is recreated with the
    // same volume, which keeps the data.
    // (container, whether it is up) of the existing container.
    let existing: Option<(ContainerInfo, bool)> = match existing {
        Some(c) if running => Some((c, true)),
        Some(c) if matches!(c.state, ContainerState::Exited | ContainerState::Created) => {
            match inner.docker.start_container(&c.id).await {
                Ok(()) => Some((c, true)),
                Err(e) => {
                    let why = redact(&error_message(&e), &ds);
                    warn!(datastore = %ds.name, "cannot start the existing container ({why}): recreating it");
                    Some((c, false))
                }
            }
        }
        Some(c) => Some((c, false)),
        None => None,
    };
    let container_id = match existing {
        Some((c, true)) => {
            // Its limits may be older than the row's: changed while the
            // datastore was failed (e.g. raised after an out-of-memory kill:
            // a restarting container gets them for its next start) or by a
            // change that raced with provisioning.
            if let Err(e) = sync_limits(inner, &mut ds, &c.id, true).await {
                warn!(datastore = %ds.name, "cannot apply the limits to the existing container: {}", redact(&error_message(&e), &ds));
            }
            c.id
        }
        other => {
            if let Some((c, _)) = other {
                inner.docker.remove_container(&c.id, true).await?;
            }
            let image = ds.image();
            // An image pull needs space too. Recreating the container of an
            // existing datastore (its data is in the volume, the image is
            // here) doesn't, and must still heal it on a full disk.
            if !new_volume && !inner.docker.image_exists(&image).await? {
                limits::check_free_disk(inner).await?;
            }
            let pull_log = LogSink::noop();
            tokio::select! {
                r = inner.docker.ensure_image(&image, &pull_log) => r?,
                _ = cancel.cancelled() => return Err(Error::Canceled),
            }
            let id = run_container(inner, &mut ds).await?;
            // Limits changed while the container was being created (the
            // change found no container to update): apply them now.
            sync_limits(inner, &mut ds, &id, true).await?;
            id
        }
    };

    wait_ready(inner, &ds, &container_id, cancel).await?;
    if ds.status != DatastoreStatus::Available || ds.error.is_some() {
        ds.status = DatastoreStatus::Available;
        ds.error = None;
        inner.store.update_datastore(&ds).await?;
        info!(datastore = %ds.name, "datastore available");
    }
    Ok(())
}

/// A datastore never adopts a volume it did not create: a volume with its
/// name left by something else (another server with the same prefix, a
/// reset data directory, a manual `docker volume create`) holds data the new
/// datastore's credentials can't open (Postgres keeps the password it was
/// initialized with) and that must not be exposed under the new name.
/// Returns whether the volume is missing (a new datastore) or its own.
async fn check_volume_owner(inner: &Inner, ds: &Datastore) -> Result<VolumeOwner> {
    match volume_owner(inner, ds).await? {
        owner @ (VolumeOwner::Missing | VolumeOwner::Datastore) => Ok(owner),
        VolumeOwner::Foreign => {
            let name = inner.naming.datastore_volume(&ds.name);
            Err(Error::conflict(format!(
                "a Docker volume named {name} already exists and was not created for this datastore \
                 (it may hold another database's data): remove it with `docker volume rm {name}` \
                 (or pick another name), then delete and create the datastore again"
            )))
        }
    }
}

/// Who the datastore's data volume (by name) belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolumeOwner {
    Missing,
    /// Created by this server for this datastore (labels
    /// `ferry.datastore=<id>` and `ferry.instance=<prefix>`).
    Datastore,
    /// Anything else: another server, an earlier datastore of the same name,
    /// a manual `docker volume create`...
    Foreign,
}

async fn volume_owner(inner: &Inner, ds: &Datastore) -> Result<VolumeOwner> {
    let name = inner.naming.datastore_volume(&ds.name);
    if !inner.docker.volume_exists(&name).await? {
        return Ok(VolumeOwner::Missing);
    }
    let volume = match inner.docker.bollard().inspect_volume(&name).await {
        Ok(v) => v,
        // Removed between the two calls.
        Err(_) if !inner.docker.volume_exists(&name).await? => return Ok(VolumeOwner::Missing),
        Err(e) => return Err(Error::Docker(format!("inspecting volume {name}: {e}"))),
    };
    let label = |k: &str| volume.labels.get(k).map(String::as_str);
    Ok(if label(LABEL_DATASTORE) == Some(ds.id.as_str()) && label(LABEL_INSTANCE) == Some(inner.naming.prefix()) {
        VolumeOwner::Datastore
    } else {
        VolumeOwner::Foreign
    })
}

/// Create the container (with the row's limits); if the stored host port
/// was taken meanwhile, allocate a new one once.
async fn run_container(inner: &Inner, ds: &mut Datastore) -> Result<String> {
    let bind_ip = inner.config.datastore_bind_ip.clone();
    let port = ds.host_port.ok_or_else(|| Error::internal("datastore host port not allocated"))?;
    let resources = resources(inner, ds).await;
    match inner.docker.run_container(&datastore_spec(&inner.naming, ds, &bind_ip, port, &resources)).await {
        Ok(info) => Ok(info.id),
        Err(e) if is_port_conflict(&e) => {
            let new_port = free_host_port()?;
            warn!(datastore = %ds.name, old = port, new = new_port, "host port taken: allocating another one");
            ds.host_port = Some(new_port);
            *ds = inner.store.update_datastore(ds).await?;
            let spec = datastore_spec(&inner.naming, ds, &bind_ip, new_port, &resources);
            let info = inner.docker.run_container(&spec).await?;
            Ok(info.id)
        }
        Err(e) => Err(e),
    }
}

/// The datastore's resources (its limits, else the server defaults), with
/// a server-log warning when they were capped or exceed the host.
async fn resources(inner: &Inner, ds: &Datastore) -> Resources {
    let resources = limits::for_container(inner, ds.memory_limit_mb, ds.cpu_limit).await;
    for warning in resources.warnings() {
        warn!(datastore = %ds.name, "{warning}");
    }
    resources
}

/// Whether container `c` already has the memory and CPU limits of
/// `resources`. A CPU quota of every host CPU is how `docker update` spells
/// "no CPU limit" (see [`ferry_docker::Docker::update_limits`]).
fn has_limits(c: &ContainerInfo, resources: &Resources, host: Option<&HostInfo>) -> bool {
    let all_cpus = host.and_then(|h| h.cpus).map(|n| i64::from(n) * 1_000_000_000);
    let quota = |nano: Option<i64>| nano.filter(|n| Some(*n) != all_cpus);
    c.memory_limit_bytes == resources.limits.memory_bytes() && quota(c.nano_cpus) == quota(resources.limits.nano_cpus())
}

/// Apply the row's current limits (copied into `ds`) to the datastore's
/// container — only when they differ from the container's if
/// `only_if_changed`. The row is read under the datastore's limit lock, so
/// of two concurrent calls (provisioning, a limit change) the later one
/// applies the newest limits.
async fn sync_limits(inner: &Inner, ds: &mut Datastore, container_id: &str, only_if_changed: bool) -> Result<()> {
    let _guard = inner.datastore_limit_locks.lock(&ds.id).await;
    if let Some(fresh) = inner.store.find_datastore(&ds.id).await? {
        ds.memory_limit_mb = fresh.memory_limit_mb;
        ds.cpu_limit = fresh.cpu_limit;
    }
    if only_if_changed {
        let wanted = limits::for_container(inner, ds.memory_limit_mb, ds.cpu_limit).await;
        match inner.docker.inspect_container(container_id).await? {
            Some(c) if has_limits(&c, &wanted, inner.host_info().await) => return Ok(()),
            Some(_) => {}
            None => return Err(Error::not_found("container", container_id)),
        }
    }
    apply_limits(inner, ds, container_id).await
}

/// Change the limits of the datastore's container in place. Redis also gets
/// the matching `maxmemory` now (its start script sets it on later starts).
async fn apply_limits(inner: &Inner, ds: &Datastore, container_id: &str) -> Result<()> {
    let resources = resources(inner, ds).await;
    inner.docker.update_limits(container_id, resources.update()).await?;
    if ds.kind == DatastoreKind::Redis {
        set_redis_maxmemory(inner, ds, container_id, resources.limits.memory_bytes()).await;
    }
    info!(datastore = %ds.name, limits = %resources.summary(), "applied datastore limits");
    Ok(())
}

/// `CONFIG SET maxmemory` on a running Redis (best effort: one that is not
/// running gets it from [`REDIS_START`] when it starts).
async fn set_redis_maxmemory(inner: &Inner, ds: &Datastore, container_id: &str, memory_limit_bytes: Option<i64>) {
    let bytes = redis_maxmemory(memory_limit_bytes).to_string();
    let argv = ["redis-cli", "config", "set", "maxmemory", bytes.as_str()];
    let env = [("REDISCLI_AUTH", ds.password.as_str())];
    match tokio::time::timeout(PROBE_TIMEOUT, inner.docker.exec_with_env(container_id, &argv, &env)).await {
        Ok(Ok(out)) if out.exit_code == 0 && out.output.trim() == "OK" => {
            debug!(datastore = %ds.name, maxmemory = %bytes, "set Redis maxmemory");
        }
        Ok(Ok(out)) => {
            let output = redact(out.output.trim(), ds);
            warn!(datastore = %ds.name, "cannot set Redis maxmemory to {bytes}: {output}");
        }
        Ok(Err(e)) => {
            let why = redact(&error_message(&e), ds);
            debug!(datastore = %ds.name, "Redis maxmemory not set now (it is set when Redis starts): {why}");
        }
        Err(_) => warn!(datastore = %ds.name, "setting Redis maxmemory timed out (it is set when Redis starts)"),
    }
}

/// `Engine::update_datastore_limits`: apply the row's limits to the
/// datastore's container in place (no restart), whatever the datastore's
/// status: a `failed` datastore's container may still be there (restarting)
/// and provisioning keeps it. A datastore without a container yet (or with
/// one about to be recreated) gets them when it is created.
pub(crate) async fn update_limits(inner: &Arc<Inner>, datastore_id: &str) -> Result<()> {
    let mut ds = inner.store.require_datastore(datastore_id).await?;
    if inner.with_rt(|rt| rt.deleting_datastores.contains(&ds.id)) {
        return Err(Error::conflict(format!("datastore '{}' is being deleted", ds.name)));
    }
    let name = inner.naming.datastore_container(&ds.name);
    let container = match inner.docker.inspect_container(&name).await? {
        Some(c)
            if c.labels.get(LABEL_DATASTORE) == Some(&ds.id)
                && c.labels.get(LABEL_INSTANCE).map(String::as_str) == Some(inner.naming.prefix())
                // Docker refuses to update these; provisioning recreates them.
                && !matches!(c.state, ContainerState::Dead | ContainerState::Removing) =>
        {
            c
        }
        _ => {
            debug!(datastore = %ds.name, "no container to update: the limits apply when it is created");
            return Ok(());
        }
    };
    match sync_limits(inner, &mut ds, &container.id, false).await {
        // Removed meanwhile (being recreated): created with the new limits.
        Err(Error::NotFound(_)) => Ok(()),
        Err(e) => Err(Error::Docker(format!(
            "changing the limits of the {} container failed: {}",
            ds.kind,
            redact(&error_message(&e), &ds)
        ))),
        Ok(()) => Ok(()),
    }
}

fn is_port_conflict(e: &Error) -> bool {
    let m = e.to_string().to_ascii_lowercase();
    m.contains("port is already allocated") || m.contains("address already in use") || m.contains("bind for")
}

async fn wait_ready(inner: &Inner, ds: &Datastore, container_id: &str, cancel: &CancellationToken) -> Result<()> {
    let limit = MIN_READY_TIMEOUT.max(Duration::from_secs(inner.config.health_check_timeout_secs));
    let deadline = Instant::now() + limit;
    let (cmd, env) = ready_command(ds);
    let argv: Vec<&str> = cmd.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        let last: String = match inner.docker.inspect_container(container_id).await? {
            None => return Err(Error::Docker(format!("the {} container disappeared", ds.kind))),
            // Exited (or restarting) after the kernel killed it.
            Some(c) if c.oom_killed => {
                let subject = format!("the {} container", ds.kind);
                return Err(Error::Docker(limits::oom_message(&subject, c.memory_limit_bytes, "the datastore's")));
            }
            Some(c) if matches!(c.state, ContainerState::Exited | ContainerState::Dead) => {
                let tail: Vec<String> =
                    inner.docker.logs(container_id, false, Some(5)).map(|l| l.line).collect::<Vec<_>>().await;
                let code = c.exit_code.map(|c| format!(" with code {c}")).unwrap_or_default();
                let tail = redact(&tail.join(" | "), ds);
                return Err(Error::Docker(format!("the {} container exited{code}: {tail}", ds.kind)));
            }
            Some(c) if c.state.is_running() => {
                match tokio::time::timeout(PROBE_TIMEOUT, inner.docker.exec_with_env(container_id, &argv, &env)).await {
                    Ok(Ok(out)) if is_ready(ds.kind, out.exit_code, &out.output) => return Ok(()),
                    Ok(Ok(out)) => redact(&out.output.trim().chars().take(200).collect::<String>(), ds),
                    Ok(Err(e)) => format!("readiness probe failed: {}", redact(&error_message(&e), ds)),
                    Err(_) => "readiness probe timed out".into(),
                }
            }
            Some(c) => format!("container is {}", c.state.as_str()),
        };
        if Instant::now() >= deadline {
            return Err(Error::Docker(format!("{} did not become ready within {}s: {last}", ds.kind, limit.as_secs())));
        }
        tokio::select! {
            _ = tokio::time::sleep(READY_POLL) => {}
            _ = cancel.cancelled() => return Err(Error::Canceled),
        }
    }
}

/// `Engine::delete_datastore`: container, then volume, then the row. The
/// volume is removed only if it was created for this datastore (see
/// [`VolumeOwner`]): a datastore that failed because a foreign volume had its
/// name never deletes someone else's data.
pub(crate) async fn delete(inner: &Arc<Inner>, datastore_id: &str) -> Result<()> {
    let ds = inner.store.require_datastore(datastore_id).await?;
    let Some(_deleting) = SetGuard::insert(inner, &ds.id, deleting_datastores_set) else {
        return Err(Error::conflict(format!("datastore '{}' is already being deleted", ds.name)));
    };
    if let Some(token) = inner.with_rt(|rt| rt.provisioning.get(&ds.id).cloned()) {
        token.cancel();
    }
    let _lock = inner.datastore_locks.lock(&ds.id).await;
    let name = inner.naming.datastore_container(&ds.name);
    if let Some(c) = inner.docker.inspect_container(&name).await?
        && c.labels.get(LABEL_DATASTORE) == Some(&ds.id)
        && c.labels.get(LABEL_INSTANCE).map(String::as_str) == Some(inner.naming.prefix())
    {
        instances::retire(inner, &[c], STOP_GRACE_SECS).await?;
    }
    let volume = inner.naming.datastore_volume(&ds.name);
    match volume_owner(inner, &ds).await? {
        VolumeOwner::Datastore => instances::remove_volume_retrying(inner, &volume).await?,
        VolumeOwner::Missing => {}
        VolumeOwner::Foreign => warn!(
            datastore = %ds.name,
            volume = %volume,
            "not removing Docker volume {volume}: it was not created for this datastore (its labels don't say \
             ferry.datastore={} and ferry.instance={}); remove it yourself if its data is not needed",
            ds.id,
            inner.naming.prefix()
        ),
    }
    inner.store.delete_datastore(&ds.id).await?;
    inner.datastore_locks.forget(&ds.id);
    inner.datastore_limit_locks.forget(&ds.id);
    inner.with_rt(|rt| rt.datastore_retries.remove(&ds.id));
    info!(datastore = %ds.name, "deleted datastore");
    Ok(())
}

/// Boot: datastores still `creating` are provisioned again.
pub(crate) async fn resume_provisioning(inner: &Arc<Inner>) -> Result<()> {
    for ds in inner.store.list_datastores().await? {
        if ds.status == DatastoreStatus::Creating
            && let Err(e) = provision(inner, &ds.id).await
        {
            warn!(datastore = %ds.name, "cannot resume provisioning: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_spec_and_probe() {
        let naming = Naming::new("ferry");
        let mut ds = Datastore::new("main-db", DatastoreKind::Postgres);
        ds.password = "pw".into();
        ds.memory_limit_mb = Some(1024);
        let resources = Resources::new(&ferry_core::Config::default(), None, ds.memory_limit_mb, ds.cpu_limit);
        let spec = datastore_spec(&naming, &ds, "127.0.0.1", 15432, &resources);
        assert_eq!(spec.name, "ferry-ds-main-db");
        // Its own memory limit, the default CPU limit, pids limit, log rotation.
        assert_eq!(spec.memory_limit_bytes, Some(1 << 30));
        assert_eq!(spec.nano_cpus, Some(1_000_000_000));
        assert_eq!(spec.pids_limit, Some(1024));
        assert_eq!(spec.log_rotation, Some(ferry_docker::LogRotation { max_size_mb: 10, max_files: 3 }));
        assert_eq!(spec.image, "postgres:16-alpine");
        assert!(spec.env.contains(&("POSTGRES_USER".into(), "main_db".into())));
        assert!(spec.env.contains(&("POSTGRES_PASSWORD".into(), "pw".into())));
        assert!(spec.env.contains(&("POSTGRES_DB".into(), "main_db".into())));
        assert_eq!((spec.cmd, spec.entrypoint), (None, None));
        assert_eq!(spec.network_aliases, vec!["main-db"]);
        assert_eq!(
            spec.publish,
            Some(PortPublish { container_port: 5432, host_ip: "127.0.0.1".into(), host_port: Some(15432) })
        );
        assert_eq!(
            spec.volumes,
            vec![VolumeMount { volume: "ferry-ds-main-db-data".into(), target: "/var/lib/postgresql/data".into() }]
        );
        assert_eq!(spec.restart_policy, RestartPolicy::UnlessStopped);
        assert_eq!(spec.labels["ferry.datastore"], ds.id);
        let (cmd, env) = ready_command(&ds);
        assert_eq!(cmd[0], "pg_isready");
        assert!(env.is_empty());
        assert!(cmd.windows(2).any(|w| w == ["-U", "main_db"]));
        assert!(cmd.windows(2).any(|w| w == ["-d", "main_db"]));
        assert!(is_ready(DatastoreKind::Postgres, 0, ""));
        assert!(!is_ready(DatastoreKind::Postgres, 2, "no response"));
    }

    #[test]
    fn redis_spec_and_probe() {
        let naming = Naming::new("ferry");
        let mut ds = Datastore::new("cache", DatastoreKind::Redis);
        ds.password = "secret".into();
        let spec = datastore_spec(&naming, &ds, "0.0.0.0", 16379, &Resources::default());
        assert_eq!((spec.memory_limit_bytes, spec.nano_cpus, spec.pids_limit), (None, None, None));
        assert_eq!(spec.image, "redis:7-alpine");
        assert_eq!(
            spec.cmd,
            Some(vec![
                "redis-server".into(),
                "--requirepass".into(),
                "secret".into(),
                "--appendonly".into(),
                "yes".into()
            ])
        );
        // Started through a script that sets maxmemory from the memory limit,
        // then runs the image's own entrypoint (which drops root).
        let entrypoint = spec.entrypoint.unwrap();
        assert_eq!(entrypoint[..2], ["/bin/sh", "-c"]);
        assert_eq!(entrypoint[2], REDIS_START);
        assert!(REDIS_START.contains("--maxmemory $((limit / 4 * 3))"));
        assert!(REDIS_START.ends_with("exec docker-entrypoint.sh \"$@\""));
        assert!(spec.env.is_empty());
        assert_eq!(spec.volumes[0].target, "/data");
        assert_eq!(spec.publish.as_ref().map(|p| p.host_ip.as_str()), Some("0.0.0.0"));
        // The password travels in the exec's environment, never in its argv.
        let (cmd, env) = ready_command(&ds);
        assert_eq!(cmd, vec!["redis-cli", "ping"]);
        assert!(cmd.iter().all(|a| !a.contains("secret")));
        assert_eq!(env, vec![("REDISCLI_AUTH", "secret".to_string())]);
        assert!(is_ready(DatastoreKind::Redis, 0, "PONG\n"));
        assert!(!is_ready(DatastoreKind::Redis, 0, "NOAUTH Authentication required."));
    }

    #[test]
    fn redis_maxmemory_leaves_headroom_below_the_limit() {
        assert_eq!(redis_maxmemory(Some(256 << 20)), 192 << 20);
        assert_eq!(redis_maxmemory(Some(3 << 30)), 2_415_919_104);
        assert_eq!(redis_maxmemory(None), 0, "unlimited");
        assert_eq!(redis_maxmemory(Some(0)), 0);
    }

    #[test]
    fn limits_are_compared_with_the_container() {
        let host = HostInfo { cpus: Some(4), ..HostInfo::default() };
        let config = ferry_core::Config::default();
        let resources = Resources::new(&config, Some(&host), Some(64), Some(0.5));
        let c = |memory: Option<i64>, nano: Option<i64>| ContainerInfo {
            id: "c1".into(),
            name: "ferry-ds-cache".into(),
            image: "redis:7-alpine".into(),
            state: ContainerState::Restarting,
            exit_code: Some(137),
            host_port: None,
            labels: Default::default(),
            started_at: None,
            restart_count: Some(3),
            oom_killed: true,
            memory_limit_bytes: memory,
            nano_cpus: nano,
        };
        assert!(has_limits(&c(Some(64 << 20), Some(500_000_000)), &resources, Some(&host)));
        assert!(!has_limits(&c(Some(32 << 20), Some(500_000_000)), &resources, Some(&host)), "older memory limit");
        assert!(!has_limits(&c(Some(64 << 20), None), &resources, Some(&host)), "no CPU limit yet");
        // Unlimited: `docker update` leaves a quota of every CPU.
        let off = ferry_core::Config { default_memory_limit_mb: 0, default_cpu_limit: 0.0, ..config };
        let unlimited = Resources::new(&off, Some(&host), None, None);
        assert!(has_limits(&c(None, None), &unlimited, Some(&host)));
        assert!(has_limits(&c(None, Some(4_000_000_000)), &unlimited, Some(&host)));
        assert!(!has_limits(&c(None, Some(4_000_000_000)), &unlimited, None), "host CPUs unknown");
        assert!(!has_limits(&c(Some(64 << 20), None), &unlimited, Some(&host)));
    }

    #[test]
    fn passwords_are_redacted() {
        let mut ds = Datastore::new("cache", DatastoreKind::Redis);
        ds.password = "S3CRET".into();
        let m = "running \"redis-cli --no-auth-warning -a S3CRET ping\" in container x: is restarting";
        let r = redact(m, &ds);
        assert!(!r.contains("S3CRET"), "{r}");
        assert!(r.contains("-a ******** ping"), "{r}");
        ds.password = String::new();
        assert_eq!(redact("nothing to hide", &ds), "nothing to hide");
    }

    #[test]
    fn port_conflicts_are_recognized() {
        assert!(is_port_conflict(&Error::Docker(
            "starting container x: Bind for 127.0.0.1:5432 failed: port is already allocated".into()
        )));
        assert!(!is_port_conflict(&Error::Docker("no such image".into())));
    }
}
