//! Managed Postgres / Redis: one container + one named volume each.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::naming::{LABEL_DATASTORE, LABEL_INSTANCE};
use ferry_core::{Datastore, DatastoreKind, DatastoreStatus, Error, LogSink, Naming, Result};
use ferry_docker::{
    ContainerInfo, ContainerSpec, ContainerState, PortPublish, RestartPolicy, VolumeMount, free_host_port,
};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::instances::{self, STOP_GRACE_SECS};
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

/// Container spec of a datastore (`host_port` must be allocated).
pub(crate) fn datastore_spec(naming: &Naming, ds: &Datastore, bind_ip: &str, host_port: u16) -> ContainerSpec {
    let (env, cmd) = match ds.kind {
        DatastoreKind::Postgres => (
            vec![
                ("POSTGRES_USER".to_string(), ds.username.clone()),
                ("POSTGRES_PASSWORD".to_string(), ds.password.clone()),
                ("POSTGRES_DB".to_string(), ds.database.clone().unwrap_or_else(|| ds.username.clone())),
            ],
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
        ),
    };
    ContainerSpec {
        name: naming.datastore_container(&ds.name),
        image: ds.image(),
        env,
        cmd,
        entrypoint: None,
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
    }
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

    check_volume_owner(inner, &ds).await?;
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
        Some((c, true)) => c.id,
        other => {
            if let Some((c, _)) = other {
                inner.docker.remove_container(&c.id, true).await?;
            }
            let image = ds.image();
            let pull_log = LogSink::noop();
            tokio::select! {
                r = inner.docker.ensure_image(&image, &pull_log) => r?,
                _ = cancel.cancelled() => return Err(Error::Canceled),
            }
            run_container(inner, &mut ds).await?
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
async fn check_volume_owner(inner: &Inner, ds: &Datastore) -> Result<()> {
    match volume_owner(inner, ds).await? {
        VolumeOwner::Missing | VolumeOwner::Datastore => Ok(()),
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

/// Create the container; if the stored host port was taken meanwhile,
/// allocate a new one once.
async fn run_container(inner: &Inner, ds: &mut Datastore) -> Result<String> {
    let bind_ip = inner.config.datastore_bind_ip.clone();
    let port = ds.host_port.ok_or_else(|| Error::internal("datastore host port not allocated"))?;
    match inner.docker.run_container(&datastore_spec(&inner.naming, ds, &bind_ip, port)).await {
        Ok(info) => Ok(info.id),
        Err(e) if is_port_conflict(&e) => {
            let new_port = free_host_port()?;
            warn!(datastore = %ds.name, old = port, new = new_port, "host port taken: allocating another one");
            ds.host_port = Some(new_port);
            *ds = inner.store.update_datastore(ds).await?;
            let info = inner.docker.run_container(&datastore_spec(&inner.naming, ds, &bind_ip, new_port)).await?;
            Ok(info.id)
        }
        Err(e) => Err(e),
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
        let spec = datastore_spec(&naming, &ds, "127.0.0.1", 15432);
        assert_eq!(spec.name, "ferry-ds-main-db");
        assert_eq!(spec.image, "postgres:16-alpine");
        assert!(spec.env.contains(&("POSTGRES_USER".into(), "main_db".into())));
        assert!(spec.env.contains(&("POSTGRES_PASSWORD".into(), "pw".into())));
        assert!(spec.env.contains(&("POSTGRES_DB".into(), "main_db".into())));
        assert_eq!(spec.cmd, None);
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
        let spec = datastore_spec(&naming, &ds, "0.0.0.0", 16379);
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
