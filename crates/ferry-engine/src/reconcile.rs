//! The reconciler: converges Docker and the proxy routes to what the store
//! says, at boot, every 10 seconds, and on demand (scale / suspend / resume
//! / after deploys). It never touches a service whose deploy is in the
//! deploying phase (that deploy holds the service lock).
//!
//! Instances are always started from the live deploy's launch spec (see
//! `spec`), never from the current settings. A route watcher follows the
//! host ports of running instances every second, so that routes follow a
//! container Docker restarted on a new port without waiting for a pass. An
//! OOM watcher logs every container of this server the kernel kills for
//! exceeding its memory limit.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::naming::{
    LABEL_DATASTORE, LABEL_DEPLOY, LABEL_INSTANCE, LABEL_JOB, LABEL_ROLE, LABEL_SERVICE, ROLE_DATASTORE, ROLE_JOB,
    ROLE_SERVICE,
};
use ferry_core::{Datastore, DatastoreStatus, Deploy, DeployStatus, Result, Service};
use ferry_docker::{ContainerInfo, ContainerState};
use futures::StreamExt;
use futures::future::join_all;
use tracing::{debug, error, info, warn};

use crate::health::{TCP_SETTLE, tcp_accepting};
use crate::instances::{self, STOP_GRACE_SECS};
use crate::spec;
use crate::state::{Inner, SetGuard, warmups_set};
use crate::util::{error_message, instance_id};

/// Interval of the periodic pass.
pub(crate) const INTERVAL: Duration = Duration::from_secs(10);
/// The first periodic pass after boot comes sooner (re-inspect what the boot
/// pass found, e.g. containers a crashed server was stopping).
const FIRST_PASS_AFTER: Duration = Duration::from_secs(2);
/// How often the route watcher lists running instances.
const ROUTE_WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// Route id of the dashboard (installed by ferryd; never removed here).
pub(crate) const DASHBOARD_ROUTE: &str = "__dashboard";
/// Back-off between retries of a failed datastore.
const DATASTORE_RETRY_MIN: Duration = Duration::from_secs(60);
const DATASTORE_RETRY_MAX: Duration = Duration::from_secs(30 * 60);
/// Services converged at the same time.
const CONCURRENCY: usize = 8;
/// Route warm-up: how often newly started instances are re-checked.
const WARMUP_POLL: Duration = Duration::from_millis(500);
/// The OOM watcher subscribes to Docker events again after this pause when
/// the stream ends (Docker restarted, connection lost).
const OOM_WATCH_RETRY: Duration = Duration::from_secs(5);

/// Which pass is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pass {
    /// At boot, before the proxy serves: only the routes are installed
    /// synchronously; the rest of each service's convergence (stopping stale
    /// instances with their grace period, starting missing ones) continues in
    /// the background, so the proxy starts serving the live instances at once.
    Boot,
    Periodic,
}

/// The periodic loop (until shutdown). Each pass runs in its own task so a
/// panic is logged instead of stopping reconciliation.
pub(crate) async fn run_loop(inner: Arc<Inner>) {
    let mut wait = FIRST_PASS_AFTER;
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
            _ = inner.reconcile_wake.notified() => {}
        }
        wait = INTERVAL;
        let pass = {
            let inner = inner.clone();
            tokio::spawn(async move { reconcile_all(&inner, Pass::Periodic).await })
        };
        let abort = pass.abort_handle();
        let outcome = tokio::select! {
            r = pass => r,
            // Nothing a pass does must finish before the server stops.
            _ = inner.shutdown.cancelled() => {
                abort.abort();
                return;
            }
        };
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("reconcile pass failed: {e}"),
            Err(e) => error!("reconcile pass panicked: {e}"),
        }
    }
}

/// One full pass.
pub(crate) async fn reconcile_all(inner: &Arc<Inner>, pass: Pass) -> Result<()> {
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
            let Some(guard) = inner.service_locks.try_lock(&svc.id) else {
                debug!(service = %svc.name, "reconcile: service busy, skipped");
                return;
            };
            if inner.is_deploying(&svc.id) {
                return;
            }
            if pass == Pass::Boot {
                if let Err(e) = refresh_routes_locked(inner, &svc.id).await {
                    warn!(service = %svc.name, "installing routes failed: {}", error_message(&e));
                }
                let (task_inner, id, name) = (inner.clone(), svc.id.clone(), svc.name.clone());
                inner.spawn(async move {
                    let _guard = guard;
                    if let Err(e) = converge_service(&task_inner, &id).await {
                        warn!(service = %name, "reconciling service failed: {}", error_message(&e));
                    }
                });
                return;
            }
            if let Err(e) = converge_service(inner, &svc.id).await {
                warn!(service = %svc.name, "reconciling service failed: {}", error_message(&e));
            }
        })
        .await;

    remove_deleted_routes(inner).await?;

    reconcile_datastores(inner, &datastores, &all).await;
    remove_orphans(inner, &services, &datastores, &all).await;
    Ok(())
}

/// Remove the routes of services that no longer exist. The routes are read
/// *before* the services: routes are only ever installed for services that
/// exist, so a route whose service is missing from the later read belongs to
/// a deleted service — never to one created during the pass (whose first
/// deploy may already have installed routes).
async fn remove_deleted_routes(inner: &Inner) -> Result<()> {
    let routes = inner.routes.snapshot();
    let known: HashSet<String> = inner.store.list_services().await?.into_iter().map(|s| s.id).collect();
    let mut removed: HashSet<&str> = HashSet::new();
    for route in &routes {
        if route.service_id != DASHBOARD_ROUTE
            && !known.contains(&route.service_id)
            && removed.insert(route.service_id.as_str())
        {
            inner.routes.remove_service(&route.service_id);
        }
    }
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
    // What the live deploy's instances run with (not the current settings).
    let spec = match &live {
        Some(live) => Some(spec::for_deploy(inner, &svc, live).await),
        None => None,
    };
    // A disk can't be shared: one instance, and stale instances go before a
    // new one starts. Otherwise the live deploy's missing instances start
    // first, so that a service never has fewer instances than it could.
    let disk = svc.disk_mount_path.is_some() || matches!(&spec, Some(Ok(s)) if s.disk_mount_path.is_some());
    let desired = if disk { svc.desired_instances().min(1) } else { svc.desired_instances() } as usize;
    let containers = instances::service_containers(inner, &svc.id, true).await?;
    let (keep, mut remove) = partition(&containers, live.as_ref().map(|d| d.id.as_str()), desired);
    remove.sort_by(|a, b| a.name.cmp(&b.name));

    // Routes first, so nothing is routed to instances about to be removed.
    update_routes(inner, &svc, &keep).await;

    let missing = desired.saturating_sub(keep.len());
    if disk {
        remove_stale(inner, &svc, &remove).await;
    }
    let started = match (&live, spec, missing) {
        (Some(live), Some(spec), 1..) => match spec {
            Ok(spec) => start_missing(inner, &svc, live, &spec, missing).await,
            Err(e) => Err(e),
        },
        _ => Ok(()),
    };
    if !disk {
        remove_stale(inner, &svc, &remove).await;
    }
    started
}

/// Remove stale / surplus instances. Instances of deploys that never went
/// live (failed, canceled, or interrupted by a crash while being health
/// checked) never served: they are removed at once instead of getting the
/// stop grace period — for a disk service that is the outage before the live
/// instance starts again. The others (a previous live deploy's, surplus
/// ones) get the grace period.
async fn remove_stale(inner: &Inner, svc: &Service, remove: &[ContainerInfo]) {
    if remove.is_empty() {
        return;
    }
    let never_live = never_live_deploys(inner, remove).await;
    let (now, graceful): (Vec<ContainerInfo>, Vec<ContainerInfo>) = remove
        .iter()
        .cloned()
        .partition(|c| c.labels.get(LABEL_DEPLOY).is_some_and(|d| never_live.contains(d.as_str())));
    info!(
        service = %svc.name,
        count = remove.len(),
        never_live = now.len(),
        "removing stale or surplus instances"
    );
    let (at_once, with_grace) =
        tokio::join!(instances::retire(inner, &now, 0), instances::retire(inner, &graceful, STOP_GRACE_SECS));
    if let Err(e) = at_once.and(with_grace) {
        warn!(service = %svc.name, "removing instances failed: {e}");
    }
}

/// The deploys (among those of `containers`) that never went live.
async fn never_live_deploys(inner: &Inner, containers: &[ContainerInfo]) -> HashSet<String> {
    let ids: HashSet<&str> = containers.iter().filter_map(|c| c.labels.get(LABEL_DEPLOY)).map(String::as_str).collect();
    let mut never_live = HashSet::new();
    for id in ids {
        match inner.store.get_deploy(id).await {
            Ok(Some(d)) if never_went_live(d.status) => {
                never_live.insert(id.to_string());
            }
            // Unknown deploys and store errors: keep the grace period.
            Ok(_) => {}
            Err(e) => debug!(deploy = id, "cannot read deploy: {e}"),
        }
    }
    never_live
}

/// Terminal statuses of deploys that never served as the live deploy.
fn never_went_live(status: DeployStatus) -> bool {
    matches!(status, DeployStatus::BuildFailed | DeployStatus::DeployFailed | DeployStatus::Canceled)
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

/// Start `count` instances of the live deploy, from its launch spec (never
/// from settings or env changed since: those need a deploy or restart).
async fn start_missing(
    inner: &Arc<Inner>,
    svc: &Service,
    live: &Deploy,
    spec: &spec::LaunchSpec,
    count: usize,
) -> Result<()> {
    let plan = spec::plan(inner, svc, spec).await?;
    if !inner.docker.image_exists(&plan.image).await? {
        warn!(
            service = %svc.name,
            image = %plan.image,
            "cannot start instances: the image of the live deploy {} no longer exists ({}): deploy again",
            live.id,
            crate::images::MissingImage::RemovedFromDocker
        );
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
/// replacement) or Docker restarted one, add them to the routes as soon as
/// they accept connections instead of waiting for the next pass. While one
/// warm-up of the service runs, another request makes it look once more
/// before it ends (the change may have come after its last look).
pub(crate) fn spawn_route_warmup(inner: &Arc<Inner>, service_id: &str) {
    let Some(guard) = SetGuard::insert(inner, service_id, warmups_set) else {
        inner.with_rt(|rt| rt.warmup_again.insert(service_id.to_string()));
        return;
    };
    let task_inner = inner.clone();
    let service_id = service_id.to_string();
    inner.spawn(async move {
        let inner = task_inner;
        let guard = guard;
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
                Ok((routable, running)) if routable >= running => {
                    // Done, unless asked to look again (checked and released
                    // together, so no request falls in between).
                    let done = inner.with_rt(|rt| {
                        if rt.warmup_again.remove(&service_id) {
                            return false;
                        }
                        rt.warmups.remove(&service_id);
                        true
                    });
                    if done {
                        guard.disarm();
                        return;
                    }
                }
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

/// Running instances of every service: service id → (container id, host
/// port) pairs, sorted.
type Fleet = HashMap<String, Vec<(String, Option<u16>)>>;

/// Follows the host ports of running instances (every
/// [`ROUTE_WATCH_INTERVAL`]). Docker restarts a crashed instance on a new
/// ephemeral host port: the routes of a service whose running instances or
/// ports changed are refreshed right away (as soon as the instances accept
/// connections) instead of at the next pass.
pub(crate) async fn watch_routes(inner: Arc<Inner>) {
    let prefix = inner.naming.prefix().to_string();
    let mut previous: Option<Fleet> = None;
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(ROUTE_WATCH_INTERVAL) => {}
        }
        let running = match inner
            .docker
            .list_containers(&[(LABEL_INSTANCE, prefix.as_str()), (LABEL_ROLE, ROLE_SERVICE)], false)
            .await
        {
            Ok(list) => list,
            Err(e) => {
                debug!("route watcher: {e}");
                continue;
            }
        };
        let fleet = fleet_of(&running);
        if let Some(previous) = &previous {
            for service_id in changed_services(previous, &fleet) {
                if !inner.is_service_deleting(&service_id) {
                    spawn_route_warmup(&inner, &service_id);
                }
            }
        }
        previous = Some(fleet);
    }
}

fn fleet_of(running: &[ContainerInfo]) -> Fleet {
    let mut fleet: Fleet = HashMap::new();
    for c in running.iter().filter(|c| c.state.is_running()) {
        if let Some(service_id) = c.labels.get(LABEL_SERVICE) {
            fleet.entry(service_id.clone()).or_default().push((c.id.clone(), c.host_port));
        }
    }
    for instances in fleet.values_mut() {
        instances.sort();
    }
    fleet
}

/// Services whose running instances (or their ports) differ.
fn changed_services(before: &Fleet, after: &Fleet) -> Vec<String> {
    let mut changed: Vec<String> =
        after.iter().filter(|(id, now)| before.get(*id) != Some(now)).map(|(id, _)| id.clone()).collect();
    changed.extend(before.keys().filter(|id| !after.contains_key(*id)).cloned());
    changed.sort();
    changed
}

/// Logs (warn) every container of this server the kernel OOM-kills, from
/// Docker's `oom` events. Docker restarts crashed instances and datastores
/// on its own and clears their `OOMKilled` flag as soon as they run again,
/// so without this an instance crash-looping on its memory limit would go
/// unnoticed. (A new instance killed during its deploy's health check fails
/// that deploy, and a killed job fails with the reason: both say so too.)
pub(crate) async fn watch_oom(inner: Arc<Inner>) {
    let filters: HashMap<String, Vec<String>> = HashMap::from([
        ("type".to_string(), vec!["container".to_string()]),
        ("event".to_string(), vec!["oom".to_string()]),
        ("label".to_string(), vec![format!("{LABEL_INSTANCE}={}", inner.naming.prefix())]),
    ]);
    // Events up to here are handled (ns since the epoch): a new subscription
    // replays what happened while there was none.
    let mut cursor: Option<i64> = None;
    loop {
        let mut options = bollard::query_parameters::EventsOptionsBuilder::default().filters(&filters);
        match cursor {
            Some(ns) => options = options.since(&events_since(ns)),
            None => cursor = Some(now_ns()),
        }
        let mut events = inner.docker.bollard().events(Some(options.build()));
        loop {
            let event = tokio::select! {
                _ = inner.shutdown.cancelled() => return,
                e = events.next() => e,
            };
            match event {
                Some(Ok(event)) => {
                    if let Some(ns) = event.time_nano.or(event.time.map(|s| s.saturating_mul(1_000_000_000))) {
                        cursor = cursor.max(Some(ns.saturating_add(1)));
                    }
                    if let Some(actor) = event.actor {
                        report_oom(&inner, actor.id.unwrap_or_default(), actor.attributes.unwrap_or_default()).await;
                    }
                }
                Some(Err(e)) => {
                    debug!("OOM watcher: {e}");
                    break;
                }
                None => break,
            }
        }
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(OOM_WATCH_RETRY) => {}
        }
    }
}

fn now_ns() -> i64 {
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since_epoch.as_nanos()).unwrap_or(i64::MAX)
}

/// Docker's `since` for a time in ns since the epoch: `<seconds>.<nanoseconds>`.
fn events_since(ns: i64) -> String {
    format!("{}.{:09}", ns.div_euclid(1_000_000_000), ns.rem_euclid(1_000_000_000))
}

/// Log one OOM kill (`attributes`: the container's name and labels).
async fn report_oom(inner: &Inner, container_id: String, attributes: HashMap<String, String>) {
    let label = |k: &str| attributes.get(k).map(String::as_str);
    let owner = match label(LABEL_ROLE) {
        Some(ROLE_SERVICE | ROLE_JOB) => match label(LABEL_SERVICE) {
            Some(id) => inner.store.get_service(id).await.ok().flatten().map(|s| s.name),
            None => None,
        },
        Some(ROLE_DATASTORE) => match label(LABEL_DATASTORE) {
            Some(id) => inner.store.find_datastore(id).await.ok().flatten().map(|d| d.name),
            None => None,
        },
        _ => None,
    };
    // The limit it hit (it is restarting, or exited: still inspectable).
    let limit = match inner.docker.inspect_container(&container_id).await {
        Ok(Some(c)) => Some(c.memory_limit_bytes),
        _ => None,
    };
    let message = oom_report(&attributes, owner.as_deref(), limit);
    warn!(container = %attributes.get("name").map_or(container_id.as_str(), String::as_str), "{message}");
}

/// The server-log line for an OOM kill. `owner`: the name of the container's
/// service / datastore; `limit`: its memory limit (`Some(None)` = none),
/// `None` if it could not be read.
fn oom_report(attributes: &HashMap<String, String>, owner: Option<&str>, limit: Option<Option<i64>>) -> String {
    let label = |k: &str| attributes.get(k).map(String::as_str);
    let name = label("name").unwrap_or("?");
    let (subject, whose) = match label(LABEL_ROLE) {
        Some(ROLE_SERVICE) => (
            format!("instance {} of service '{}'", instance_id(name), owner.or(label(LABEL_SERVICE)).unwrap_or("?")),
            "the service's",
        ),
        Some(ROLE_JOB) => (
            format!(
                "job {} of service '{}'",
                label(LABEL_JOB).unwrap_or("?"),
                owner.or(label(LABEL_SERVICE)).unwrap_or("?")
            ),
            "the service's",
        ),
        Some(ROLE_DATASTORE) => {
            (format!("datastore '{}'", owner.or(label(LABEL_DATASTORE)).unwrap_or("?")), "the datastore's")
        }
        _ => (format!("container {name}"), "its"),
    };
    match limit {
        Some(limit) => crate::limits::oom_message(&subject, limit, whose),
        None => format!("{subject} ran out of memory and was killed"),
    }
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
        let why = match container {
            None => "missing",
            Some(c) => match c.state {
                ContainerState::Exited | ContainerState::Created => "stopped",
                ContainerState::Paused => "paused",
                ContainerState::Dead => "dead",
                _ => continue,
            },
        };
        // Provisioning starts it (or recreates it with the same volume when it
        // can't be started) and records a failure on the datastore.
        info!(datastore = %ds.name, "datastore container is {why}: provisioning it again");
        if let Err(e) = crate::datastores::provision(inner, &ds.id).await {
            warn!(datastore = %ds.name, "cannot provision datastore: {e}");
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
            oom_killed: false,
            memory_limit_bytes: None,
            nano_cpus: None,
        }
    }

    fn names(v: &[ContainerInfo]) -> Vec<&str> {
        v.iter().map(|c| c.name.as_str()).collect()
    }

    /// Engine state with an in-memory store and an unreachable Docker.
    async fn test_inner() -> (tempfile::TempDir, Inner) {
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
        (dir, inner)
    }

    #[tokio::test]
    async fn routes_of_deleted_services_are_removed() {
        let (_dir, inner) = test_inner().await;
        let svc = Service::new("web", ferry_core::ServiceType::WebService);
        inner.store.create_service(&svc).await.unwrap();
        let up: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        inner.routes.set_service_routes(&svc.id, &["web.localhost".into()], vec![up]);
        inner.routes.set_service_routes("srv-gone", &["gone.localhost".into(), "gone.example.com".into()], vec![up]);
        inner.routes.set_service_routes(DASHBOARD_ROUTE, &["ferry.localhost".into()], vec![up]);
        remove_deleted_routes(&inner).await.unwrap();
        let ids: HashSet<String> = inner.routes.snapshot().into_iter().map(|r| r.service_id).collect();
        assert!(ids.contains(&svc.id), "{ids:?}");
        assert!(ids.contains(DASHBOARD_ROUTE), "{ids:?}");
        assert!(!ids.contains("srv-gone"), "{ids:?}");
    }

    #[tokio::test]
    async fn warmup_guards_release_their_key_unless_disarmed() {
        let (_dir, inner) = test_inner().await;
        let inner = Arc::new(inner);
        let g = SetGuard::insert(&inner, "srv-1", warmups_set).unwrap();
        assert!(SetGuard::insert(&inner, "srv-1", warmups_set).is_none(), "one warm-up per service");
        drop(g);
        let g = SetGuard::insert(&inner, "srv-1", warmups_set).unwrap();
        // Released by its owner, then taken by a new warm-up: the old guard
        // must leave the new one's key alone.
        inner.with_rt(|rt| rt.warmups.remove("srv-1"));
        let newer = SetGuard::insert(&inner, "srv-1", warmups_set).unwrap();
        g.disarm();
        assert!(inner.with_rt(|rt| rt.warmups.contains("srv-1")));
        drop(newer);
        assert!(!inner.with_rt(|rt| rt.warmups.contains("srv-1")));
    }

    #[tokio::test]
    async fn failed_datastores_are_retried_with_backoff() {
        let (_dir, inner) = test_inner().await;
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

    #[tokio::test]
    async fn instances_of_never_live_deploys_are_removed_without_grace() {
        let (_dir, inner) = test_inner().await;
        let svc = Service::new("dsk", ferry_core::ServiceType::WebService);
        inner.store.create_service(&svc).await.unwrap();
        let mut ids = Vec::new();
        for status in [
            DeployStatus::DeployFailed,
            DeployStatus::Canceled,
            DeployStatus::Deactivated,
            DeployStatus::Live,
            DeployStatus::BuildFailed,
        ] {
            let mut d = Deploy::new(
                &svc.id,
                ferry_core::DeployTrigger::Manual,
                ferry_core::DeploySource::Image { image: "x".into() },
            );
            d.status = status;
            inner.store.create_deploy(&d).await.unwrap();
            ids.push(d.id);
        }
        let containers: Vec<ContainerInfo> = ids
            .iter()
            .map(|id| c(id, id, ContainerState::Running))
            .chain([c("unknown", "dep-unknown", ContainerState::Running)])
            .collect();
        let never_live = never_live_deploys(&inner, &containers).await;
        let expected: HashSet<String> = [ids[0].clone(), ids[1].clone(), ids[4].clone()].into();
        assert_eq!(never_live, expected, "failed, canceled and interrupted deploys never served");
    }

    #[test]
    fn route_watcher_notices_restarts_on_new_ports() {
        let mut a = c("a", "live", ContainerState::Running);
        a.labels.insert(LABEL_SERVICE.to_string(), "srv-1".into());
        let mut b = c("b", "live", ContainerState::Running);
        b.labels.insert(LABEL_SERVICE.to_string(), "srv-2".into());
        let before = fleet_of(&[a.clone(), b.clone()]);
        assert!(changed_services(&before, &before).is_empty());
        // Docker restarted `a` on another host port.
        let mut moved = a.clone();
        moved.host_port = Some(2000);
        assert_eq!(changed_services(&before, &fleet_of(&[moved, b.clone()])), vec!["srv-1".to_string()]);
        // `b` stopped (restarting: not running), `a` unchanged.
        let mut restarting = b.clone();
        restarting.state = ContainerState::Restarting;
        assert_eq!(changed_services(&before, &fleet_of(&[a.clone(), restarting])), vec!["srv-2".to_string()]);
        // A new instance of `srv-1`.
        let mut extra = c("x", "live", ContainerState::Running);
        extra.labels.insert(LABEL_SERVICE.to_string(), "srv-1".into());
        assert_eq!(changed_services(&before, &fleet_of(&[a, b, extra])), vec!["srv-1".to_string()]);
    }

    #[test]
    fn events_resume_from_the_cursor() {
        assert_eq!(events_since(1_700_000_000_123_456_789), "1700000000.123456789");
        assert_eq!(events_since(1_700_000_000_000_000_001), "1700000000.000000001");
        assert!(now_ns() > 1_700_000_000_000_000_000);
    }

    #[test]
    fn oom_reports_name_the_owner_and_the_limit() {
        let attrs = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
        };
        let instance = attrs(&[
            ("name", "ferry-web-cdef4567-a1b2c3"),
            (LABEL_ROLE, ROLE_SERVICE),
            (LABEL_SERVICE, "srv-1"),
            (LABEL_DEPLOY, "dep-1"),
        ]);
        assert_eq!(
            oom_report(&instance, Some("web"), Some(Some(512 << 20))),
            "instance a1b2c3 of service 'web' ran out of memory (limit 512 MiB) — raise the service's memory limit"
        );
        assert_eq!(
            oom_report(&instance, None, None),
            "instance a1b2c3 of service 'srv-1' ran out of memory and was killed"
        );
        let job =
            attrs(&[("name", "ferry-job-x"), (LABEL_ROLE, ROLE_JOB), (LABEL_SERVICE, "srv-1"), (LABEL_JOB, "job-9")]);
        assert!(oom_report(&job, Some("web"), Some(Some(64 << 20))).starts_with("job job-9 of service 'web' ran out"));
        let ds = attrs(&[("name", "ferry-ds-db"), (LABEL_ROLE, ROLE_DATASTORE), (LABEL_DATASTORE, "dbs-1")]);
        assert_eq!(
            oom_report(&ds, Some("db"), Some(Some(1 << 30))),
            "datastore 'db' ran out of memory (limit 1 GiB) — raise the datastore's memory limit"
        );
        let other = attrs(&[("name", "stray")]);
        assert!(oom_report(&other, None, Some(None)).starts_with("container stray was killed by the kernel's"));
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
