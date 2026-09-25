//! The reconciler: converges Docker and the proxy routes to what the store
//! says, at boot, every 10 seconds, and on demand (scale / suspend / resume
//! / after deploys). It never touches a service whose deploy is in the
//! deploying phase (that deploy holds the service lock).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::naming::{
    LABEL_DATASTORE, LABEL_DEPLOY, LABEL_INSTANCE, LABEL_JOB, LABEL_ROLE, LABEL_SERVICE, ROLE_DATASTORE, ROLE_JOB,
    ROLE_SERVICE,
};
use ferry_core::{Datastore, DatastoreStatus, Deploy, Result, Service};
use ferry_docker::{ContainerInfo, ContainerState};
use futures::StreamExt;
use futures::future::join_all;
use tracing::{debug, error, info, warn};

use crate::health::{TCP_SETTLE, tcp_accepting};
use crate::instances::{self, STOP_GRACE_SECS};
use crate::state::{Inner, SetGuard, warmups_set};
use crate::util::{error_message, instance_id};

/// Interval of the periodic pass.
pub(crate) const INTERVAL: Duration = Duration::from_secs(10);
/// Route id of the dashboard (installed by ferryd; never removed here).
pub(crate) const DASHBOARD_ROUTE: &str = "__dashboard";
/// Back-off between retries of a failed datastore.
const DATASTORE_RETRY_MIN: Duration = Duration::from_secs(60);
const DATASTORE_RETRY_MAX: Duration = Duration::from_secs(30 * 60);
/// Services converged at the same time.
const CONCURRENCY: usize = 8;
/// Route warm-up: how often newly started instances are re-checked.
const WARMUP_POLL: Duration = Duration::from_millis(500);

/// The periodic loop (until shutdown). Each pass runs in its own task so a
/// panic is logged instead of stopping reconciliation.
pub(crate) async fn run_loop(inner: Arc<Inner>) {
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(INTERVAL) => {}
            _ = inner.reconcile_wake.notified() => {}
        }
        let pass = {
            let inner = inner.clone();
            tokio::spawn(async move { reconcile_all(&inner).await })
        };
        match pass.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("reconcile pass failed: {e}"),
            Err(e) => error!("reconcile pass panicked: {e}"),
        }
    }
}

/// One full pass.
pub(crate) async fn reconcile_all(inner: &Arc<Inner>) -> Result<()> {
    let _pass = inner.reconcile_lock.lock().await;
    let prefix = inner.naming.prefix().to_string();
    // List containers *before* reading the store: a container that existed at
    // listing time whose owner is gone from the store afterwards is an orphan.
    let all = inner.docker.list_containers(&[(LABEL_INSTANCE, prefix.as_str())], true).await?;
    let services = inner.store.list_services().await?;
    let datastores = inner.store.list_datastores().await?;

    // Services are independent (each has its own lock): converge several at
    // once so route probes of many services don't add up.
    futures::stream::iter(services.iter())
        .for_each_concurrent(CONCURRENCY, |svc| async move {
            if inner.is_service_deleting(&svc.id) {
                return;
            }
            // Busy (deploying, scaling, being suspended...): next pass.
            let Some(_guard) = inner.service_locks.try_lock(&svc.id) else {
                debug!(service = %svc.name, "reconcile: service busy, skipped");
                return;
            };
            if inner.is_deploying(&svc.id) {
                return;
            }
            if let Err(e) = converge_service(inner, &svc.id).await {
                warn!(service = %svc.name, "reconciling service failed: {}", error_message(&e));
            }
        })
        .await;

    let known: HashSet<&str> = services.iter().map(|s| s.id.as_str()).collect();
    for route in inner.routes.snapshot() {
        if route.service_id != DASHBOARD_ROUTE && !known.contains(route.service_id.as_str()) {
            inner.routes.remove_service(&route.service_id);
        }
    }

    reconcile_datastores(inner, &datastores, &all).await;
    remove_orphans(inner, &services, &datastores, &all).await;
    Ok(())
}

/// Converge one service. The caller holds the service lock.
pub(crate) async fn converge_service(inner: &Arc<Inner>, service_id: &str) -> Result<()> {
    let Some(svc) = inner.store.get_service(service_id).await? else {
        return Ok(());
    };
    let live = match &svc.live_deploy_id {
        Some(id) => inner.store.get_deploy(id).await?,
        None => None,
    };
    let containers = instances::service_containers(inner, &svc.id, true).await?;
    let desired = svc.desired_instances() as usize;
    let (keep, mut remove) = partition(&containers, live.as_ref().map(|d| d.id.as_str()), desired);

    // Routes first, so nothing is routed to instances about to be removed.
    update_routes(inner, &svc, &keep).await;

    if !remove.is_empty() {
        info!(service = %svc.name, count = remove.len(), "removing stale or surplus instances");
        remove.sort_by(|a, b| a.name.cmp(&b.name));
        if let Err(e) = instances::retire(inner, &remove, STOP_GRACE_SECS).await {
            warn!(service = %svc.name, "removing instances failed: {e}");
        }
    }

    let missing = desired.saturating_sub(keep.len());
    if missing > 0
        && let Some(live) = &live
    {
        start_missing(inner, &svc, live, missing).await?;
    }
    Ok(())
}

/// Split a service's containers into the ones to keep (running-ish
/// containers of the live deploy, at most `desired`, running ones first) and
/// the ones to remove (other deploys, exited/dead/created, surplus).
pub(crate) fn partition(
    containers: &[ContainerInfo],
    live_id: Option<&str>,
    desired: usize,
) -> (Vec<ContainerInfo>, Vec<ContainerInfo>) {
    let mut keep = Vec::new();
    let mut remove = Vec::new();
    for c in containers {
        let of_live = live_id.is_some() && c.labels.get(LABEL_DEPLOY).map(String::as_str) == live_id;
        match c.state {
            // Being removed already: leave it alone.
            ContainerState::Removing => {}
            ContainerState::Exited | ContainerState::Dead | ContainerState::Created => remove.push(c.clone()),
            _ if !of_live => remove.push(c.clone()),
            // Running, restarting (crash loop: Docker keeps restarting it),
            // paused or transitional: count it.
            _ => keep.push(c.clone()),
        }
    }
    keep.sort_by(|a, b| b.state.is_running().cmp(&a.state.is_running()).then_with(|| a.name.cmp(&b.name)));
    if keep.len() > desired {
        remove.extend(keep.split_off(desired));
    }
    (keep, remove)
}

/// Upstreams of a service: running containers with a published port that
/// accept TCP connections (probed in parallel).
async fn routable(containers: &[ContainerInfo]) -> Vec<SocketAddr> {
    let candidates: Vec<u16> = containers.iter().filter(|c| c.state.is_running()).filter_map(|c| c.host_port).collect();
    let checks = join_all(candidates.iter().map(|p| tcp_accepting(*p, TCP_SETTLE))).await;
    candidates
        .into_iter()
        .zip(checks)
        .filter(|(_, ok)| *ok)
        .map(|(p, _)| SocketAddr::from(([127, 0, 0, 1], p)))
        .collect()
}

/// Install the service's routes from its current (kept) containers. Returns
/// (routable, running) instance counts.
async fn update_routes(inner: &Inner, svc: &Service, keep: &[ContainerInfo]) -> (usize, usize) {
    let hosts = inner.config.service_hosts(svc);
    let running = keep.iter().filter(|c| c.state.is_running()).count();
    if hosts.is_empty() {
        inner.routes.remove_service(&svc.id);
        return (running, running);
    }
    if svc.suspended {
        inner.routes.set_service_suspended(&svc.id, &hosts);
        return (0, 0);
    }
    let upstreams = routable(keep).await;
    let n = upstreams.len();
    inner.routes.set_service_routes(&svc.id, &hosts, upstreams);
    (n, running)
}

/// Start `count` instances of the live deploy (same spec as the deploy).
async fn start_missing(inner: &Arc<Inner>, svc: &Service, live: &Deploy, count: usize) -> Result<()> {
    let plan = instances::plan_for_deploy(inner, svc, live).await?;
    if !inner.docker.image_exists(&plan.image).await? {
        warn!(service = %svc.name, image = %plan.image, "cannot start instances: the live image no longer exists");
        return Ok(());
    }
    info!(service = %svc.name, deploy = %live.id, count, "starting missing instances");
    let mut started = 0;
    for _ in 0..count {
        let spec = instances::service_spec(&inner.naming, svc, &live.id, &plan);
        match inner.docker.run_container(&spec).await {
            Ok(info) => {
                started += 1;
                debug!(service = %svc.name, instance = %instance_id(&info.name), "started instance");
            }
            Err(e) => {
                warn!(service = %svc.name, "starting an instance failed: {e}");
                break;
            }
        }
    }
    if started > 0 && svc.is_public_http() {
        spawn_route_warmup(inner, &svc.id);
    }
    Ok(())
}

/// After instances were started outside a deploy (resume, scale, crash
/// replacement), add them to the routes as soon as they accept connections
/// instead of waiting for the next pass.
pub(crate) fn spawn_route_warmup(inner: &Arc<Inner>, service_id: &str) {
    let Some(guard) = SetGuard::insert(inner, service_id, warmups_set) else {
        return;
    };
    let inner = inner.clone();
    let service_id = service_id.to_string();
    tokio::spawn(async move {
        let _guard = guard;
        let deadline = Instant::now() + Duration::from_secs(inner.config.health_check_timeout_secs.max(10));
        while Instant::now() < deadline {
            tokio::select! {
                _ = inner.shutdown.cancelled() => return,
                _ = tokio::time::sleep(WARMUP_POLL) => {}
            }
            let Some(_lock) = inner.service_locks.try_lock(&service_id) else { continue };
            if inner.is_deploying(&service_id) {
                return;
            }
            match refresh_routes_locked(&inner, &service_id).await {
                Ok((routable, running)) if routable >= running => return,
                Ok(_) => {}
                Err(e) => {
                    debug!(service = %service_id, "route warm-up: {e}");
                    return;
                }
            }
        }
    });
}

/// Recompute a service's routes from Docker. The caller holds the service
/// lock. Returns (routable, running) instance counts of the live deploy.
pub(crate) async fn refresh_routes_locked(inner: &Inner, service_id: &str) -> Result<(usize, usize)> {
    let Some(svc) = inner.store.get_service(service_id).await? else {
        inner.routes.remove_service(service_id);
        return Ok((0, 0));
    };
    // No Docker needed for services without routes or suspended ones.
    let hosts = inner.config.service_hosts(&svc);
    if hosts.is_empty() {
        inner.routes.remove_service(&svc.id);
        return Ok((0, 0));
    }
    if svc.suspended {
        inner.routes.set_service_suspended(&svc.id, &hosts);
        return Ok((0, 0));
    }
    let containers = instances::service_containers(inner, &svc.id, false).await?;
    let (keep, _) = partition(&containers, svc.live_deploy_id.as_deref(), usize::MAX);
    Ok(update_routes(inner, &svc, &keep).await)
}

/// Re-install a service's routes with its current hosts but the upstreams
/// the proxy already has (used while a deploy owns the service).
pub(crate) fn rehost_routes(inner: &Inner, svc: &Service) {
    let hosts = inner.config.service_hosts(svc);
    if hosts.is_empty() {
        inner.routes.remove_service(&svc.id);
        return;
    }
    if svc.suspended {
        inner.routes.set_service_suspended(&svc.id, &hosts);
        return;
    }
    let mut upstreams: Vec<SocketAddr> = Vec::new();
    for r in inner.routes.snapshot().into_iter().filter(|r| r.service_id == svc.id) {
        for u in r.upstreams {
            if !upstreams.contains(&u) {
                upstreams.push(u);
            }
        }
    }
    inner.routes.set_service_routes(&svc.id, &hosts, upstreams);
}

/// Available datastores must have a running container (volumes keep data);
/// failed ones are retried with an exponential back-off.
async fn reconcile_datastores(inner: &Arc<Inner>, datastores: &[Datastore], all: &[ContainerInfo]) {
    let now = Instant::now();
    for ds in datastores {
        let busy = inner.with_rt(|rt| rt.provisioning.contains_key(&ds.id) || rt.deleting_datastores.contains(&ds.id));
        if busy {
            continue;
        }
        match ds.status {
            DatastoreStatus::Available => {
                inner.with_rt(|rt| rt.datastore_retries.remove(&ds.id));
            }
            DatastoreStatus::Failed => {
                if retry_due(inner, &ds.id, now) {
                    info!(datastore = %ds.name, "retrying failed datastore");
                    if let Err(e) = crate::datastores::provision(inner, &ds.id).await {
                        warn!(datastore = %ds.name, "cannot retry datastore: {e}");
                    }
                }
                continue;
            }
            // Being provisioned (or about to be, at boot).
            DatastoreStatus::Creating => continue,
        }
        let name = inner.naming.datastore_container(&ds.name);
        let container = all.iter().find(|c| {
            c.name == name
                || (c.labels.get(LABEL_ROLE).map(String::as_str) == Some(ROLE_DATASTORE)
                    && c.labels.get(LABEL_DATASTORE) == Some(&ds.id))
        });
        match container {
            None => {
                info!(datastore = %ds.name, "datastore container is missing: recreating it");
                if let Err(e) = crate::datastores::provision(inner, &ds.id).await {
                    warn!(datastore = %ds.name, "cannot recreate datastore: {e}");
                }
            }
            Some(c) if matches!(c.state, ContainerState::Exited | ContainerState::Created) => {
                info!(datastore = %ds.name, "datastore container is stopped: starting it");
                if let Err(e) = inner.docker.start_container(&c.id).await {
                    warn!(datastore = %ds.name, "cannot start datastore container: {e}");
                }
            }
            Some(c) if c.state == ContainerState::Dead => {
                info!(datastore = %ds.name, "datastore container is dead: recreating it");
                if let Err(e) = crate::datastores::provision(inner, &ds.id).await {
                    warn!(datastore = %ds.name, "cannot recreate datastore: {e}");
                }
            }
            Some(_) => {}
        }
    }
}

/// Whether a failed datastore may be retried now (and schedule the next try).
fn retry_due(inner: &Inner, id: &str, now: Instant) -> bool {
    inner.with_rt(|rt| match rt.datastore_retries.get_mut(id) {
        Some((next, _)) if now < *next => false,
        Some((next, backoff)) => {
            *backoff = (*backoff * 2).min(DATASTORE_RETRY_MAX);
            *next = now + *backoff;
            true
        }
        None => {
            rt.datastore_retries.insert(id.to_string(), (now + DATASTORE_RETRY_MIN, DATASTORE_RETRY_MIN));
            true
        }
    })
}

/// Containers of this server whose owner no longer exists.
async fn remove_orphans(inner: &Arc<Inner>, services: &[Service], datastores: &[Datastore], all: &[ContainerInfo]) {
    let service_ids: HashSet<&str> = services.iter().map(|s| s.id.as_str()).collect();
    let datastore_ids: HashSet<&str> = datastores.iter().map(|d| d.id.as_str()).collect();
    let mut orphans: Vec<ContainerInfo> = Vec::new();
    for c in all {
        let label = |k: &str| c.labels.get(k).map(String::as_str);
        match label(LABEL_ROLE) {
            Some(ROLE_SERVICE) if label(LABEL_SERVICE).is_none_or(|id| !service_ids.contains(id)) => {
                orphans.push(c.clone());
            }
            Some(ROLE_DATASTORE) if label(LABEL_DATASTORE).is_none_or(|id| !datastore_ids.contains(id)) => {
                orphans.push(c.clone());
            }
            Some(ROLE_JOB) => {
                let Some(job_id) = label(LABEL_JOB) else {
                    orphans.push(c.clone());
                    continue;
                };
                if inner.with_rt(|rt| rt.jobs.contains_key(job_id)) {
                    continue;
                }
                match inner.store.get_job_run(job_id).await {
                    Ok(Some(j)) if !j.status.is_terminal() => {}
                    Ok(_) => orphans.push(c.clone()),
                    Err(e) => debug!(job = job_id, "cannot check job container: {e}"),
                }
            }
            // Not ours to judge (unknown role).
            _ => {}
        }
    }
    if orphans.is_empty() {
        return;
    }
    for c in &orphans {
        info!(container = %c.name, "removing orphaned container");
    }
    if let Err(e) = instances::retire(inner, &orphans, 0).await {
        warn!("removing orphaned containers failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn c(name: &str, deploy: &str, state: ContainerState) -> ContainerInfo {
        ContainerInfo {
            id: format!("id-{name}"),
            name: name.to_string(),
            image: "img".into(),
            state,
            exit_code: None,
            host_port: Some(1000),
            labels: BTreeMap::from([(LABEL_DEPLOY.to_string(), deploy.to_string())]),
            started_at: None,
            restart_count: None,
        }
    }

    fn names(v: &[ContainerInfo]) -> Vec<&str> {
        v.iter().map(|c| c.name.as_str()).collect()
    }

    #[tokio::test]
    async fn failed_datastores_are_retried_with_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(ferry_core::Config { data_dir: dir.path().to_path_buf(), ..Default::default() });
        let store = ferry_core::Store::open_in_memory().await.unwrap();
        let bollard =
            bollard::Docker::connect_with_http("http://127.0.0.1:9", 2, bollard::API_DEFAULT_VERSION).unwrap();
        let builder = ferry_build::Builder::new(config.builds_dir(), config.repos_dir(), "docker".into());
        let inner = Inner::new(
            config,
            store,
            ferry_docker::Docker::from_bollard(bollard),
            builder,
            ferry_proxy::RouteTable::new(),
        );
        let t0 = Instant::now();
        assert!(retry_due(&inner, "dbs-1", t0), "first retry right away");
        assert!(!retry_due(&inner, "dbs-1", t0 + Duration::from_secs(59)));
        assert!(retry_due(&inner, "dbs-1", t0 + DATASTORE_RETRY_MIN));
        // The back-off doubled: 2 minutes after that retry.
        let t1 = t0 + DATASTORE_RETRY_MIN;
        assert!(!retry_due(&inner, "dbs-1", t1 + Duration::from_secs(119)));
        assert!(retry_due(&inner, "dbs-1", t1 + Duration::from_secs(120)));
        let (_, backoff) = inner.with_rt(|rt| rt.datastore_retries["dbs-1"]);
        assert_eq!(backoff, Duration::from_secs(240));
        for i in 0..20 {
            retry_due(&inner, "dbs-1", t1 + Duration::from_secs(3600 * (i + 1)));
        }
        let (_, backoff) = inner.with_rt(|rt| rt.datastore_retries["dbs-1"]);
        assert_eq!(backoff, DATASTORE_RETRY_MAX);
        assert!(retry_due(&inner, "dbs-2", t0), "tracked per datastore");
    }

    #[test]
    fn partition_keeps_live_running_up_to_desired() {
        let containers = vec![
            c("c", "live", ContainerState::Running),
            c("a", "live", ContainerState::Restarting),
            c("b", "live", ContainerState::Running),
            c("d", "old", ContainerState::Running),
            c("e", "live", ContainerState::Exited),
            c("f", "live", ContainerState::Created),
            c("g", "live", ContainerState::Removing),
            c("h", "live", ContainerState::Dead),
        ];
        let (keep, remove) = partition(&containers, Some("live"), 2);
        assert_eq!(names(&keep), ["b", "c"], "running ones first, by name");
        let mut removed = names(&remove);
        removed.sort();
        assert_eq!(removed, ["a", "d", "e", "f", "h"]);

        let (keep, _) = partition(&containers, Some("live"), 10);
        assert_eq!(names(&keep), ["b", "c", "a"], "a crash-looping instance still counts");

        // Suspended / cron: desired 0 → everything goes.
        let (keep, remove) = partition(&containers, Some("live"), 0);
        assert!(keep.is_empty());
        assert_eq!(remove.len(), 7);

        // No live deploy: nothing is kept.
        let (keep, remove) = partition(&containers, None, 3);
        assert!(keep.is_empty());
        assert_eq!(remove.len(), 7);
    }
}
