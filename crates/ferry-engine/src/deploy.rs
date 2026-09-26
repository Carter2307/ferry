//! The deploy queue: enqueueing (deploy / rollback / restart), superseding,
//! one worker per service, cancellation and boot recovery.
//!
//! Queue changes (insert + supersede, cancel of a queued deploy, a worker
//! claiming the next deploy or exiting) all happen under
//! `Inner::queue_lock`, so a deploy can never be claimed twice, canceled
//! while being claimed, or left behind by an exiting worker.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ferry_core::naming::{LABEL_DEPLOY, LABEL_INSTANCE};
use ferry_core::{
    Deploy, DeployRequest, DeploySource, DeployStatus, DeployTrigger, Error, JobStatus, Result, Service, SourceKind,
};
use tokio::sync::{Notify, OwnedSemaphorePermit, watch};
use tracing::{error, info, warn};

use crate::logs::LogKind;
use crate::pipeline;
use crate::state::{ActiveDeploy, DeployOptions, Inner, Worker};
use crate::util::{error_message, panic_message};

/// How long `cancel_deploy` (and suspend/delete) wait for a running deploy
/// to wind down.
const CANCEL_WAIT: Duration = Duration::from_secs(60);
/// Pause after a store error in a worker before retrying.
const WORKER_RETRY: Duration = Duration::from_secs(2);
/// Deploy rows scanned when looking for the latest uploaded archive.
const ARCHIVE_SCAN_LIMIT: u32 = 10_000;

pub(crate) const INTERRUPTED: &str = "interrupted by server restart";
pub(crate) const SHUTDOWN: &str = "interrupted by server shutdown";

/// `Engine::deploy`.
pub(crate) async fn deploy(inner: &Arc<Inner>, service_id: &str, req: DeployRequest) -> Result<Deploy> {
    let svc = inner.store.require_service(service_id).await?;
    check_deployable(inner, &svc)?;
    let source = resolve_source(inner, &svc, &req).await?;
    enqueue(inner, &svc, req.trigger, source, DeployOptions { clear_cache: req.clear_cache }, false).await
}

/// `Engine::rollback`.
pub(crate) async fn rollback(inner: &Arc<Inner>, service_id: &str, deploy_id: &str) -> Result<Deploy> {
    let svc = inner.store.require_service(service_id).await?;
    check_deployable(inner, &svc)?;
    let target = inner.store.require_deploy(deploy_id.trim()).await?;
    if target.service_id != svc.id {
        return Err(Error::invalid(format!("deploy {} does not belong to service '{}'", target.id, svc.name)));
    }
    let image = target.image.clone().ok_or_else(|| {
        Error::invalid(format!("deploy {} has no image to roll back to (it never finished building)", target.id))
    })?;
    // Like Render: only versions that actually served can be rolled back to.
    if !matches!(target.status, DeployStatus::Live | DeployStatus::Deactivated) {
        return Err(Error::invalid(format!(
            "deploy {} never went live ({}): only deploys that were live can be rolled back to",
            target.id, target.status
        )));
    }
    if !inner.docker.image_exists(&image).await? {
        return Err(Error::invalid(format!(
            "the image of deploy {} ({image}) no longer exists: only the newest {} built images of a service are kept",
            target.id, inner.config.keep_images
        )));
    }
    let source = DeploySource::Reuse { image, from_deploy: Some(target.id.clone()) };
    enqueue(inner, &svc, DeployTrigger::Rollback, source, DeployOptions::default(), false).await
}

/// `Engine::restart`: a deploy that reuses the live image with the current
/// env and settings. While a first deploy is still in progress (nothing
/// live yet), the restart is queued behind it and restarts it once live.
pub(crate) async fn restart(inner: &Arc<Inner>, service_id: &str, trigger: DeployTrigger) -> Result<Deploy> {
    let svc = inner.store.require_service(service_id).await?;
    check_deployable(inner, &svc)?;
    let target = match &svc.live_deploy_id {
        Some(live_id) => Some(inner.store.require_deploy(live_id).await?),
        None => inner.store.active_deploys().await?.into_iter().rev().find(|d| d.service_id == svc.id),
    };
    let Some(target) = target else {
        return Err(Error::conflict(format!("service '{}' has no live deploy to restart: deploy it first", svc.name)));
    };
    if svc.live_deploy_id.is_some() && target.image.is_none() {
        return Err(Error::conflict(format!("the live deploy {} of '{}' has no image", target.id, svc.name)));
    }
    // The image is re-resolved when the restart runs (see the pipeline), so
    // a deploy that goes live in between is not rolled back.
    let source =
        DeploySource::Reuse { image: target.image.clone().unwrap_or_default(), from_deploy: Some(target.id.clone()) };
    // A deploy that is already queued starts with the current env and
    // settings too: superseding it with a restart would drop its new code.
    enqueue(inner, &svc, trigger, source, DeployOptions::default(), true).await
}

/// Restarts reuse whatever is live when they run (not when they were queued).
pub(crate) fn is_restart(trigger: DeployTrigger) -> bool {
    matches!(trigger, DeployTrigger::Restart | DeployTrigger::EnvChange)
}

fn check_deployable(inner: &Inner, svc: &Service) -> Result<()> {
    if inner.is_service_deleting(&svc.id) {
        return Err(Error::conflict(format!("service '{}' is being deleted", svc.name)));
    }
    if svc.suspended {
        return Err(Error::conflict(format!("service '{}' is suspended: resume it before deploying", svc.name)));
    }
    Ok(())
}

/// The source of a deploy: explicit, else derived from the service (image,
/// git repo, or the most recent uploaded archive).
async fn resolve_source(inner: &Inner, svc: &Service, req: &DeployRequest) -> Result<DeploySource> {
    let commit = req.commit.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
    let source = match &req.source {
        Some(DeploySource::Git { repo_url, branch, commit: explicit }) => DeploySource::Git {
            repo_url: repo_url.clone(),
            branch: branch.clone(),
            commit: explicit.clone().or(commit.clone()),
        },
        Some(other) => other.clone(),
        None => match svc.source_kind() {
            SourceKind::Image => DeploySource::Image { image: svc.image.clone().unwrap_or_default() },
            SourceKind::Git => DeploySource::Git {
                repo_url: svc.repo_url.clone().unwrap_or_default(),
                branch: svc.branch.clone(),
                commit: commit.clone(),
            },
            SourceKind::Upload => latest_archive(inner, svc).await?,
        },
    };
    if commit.is_some() && !matches!(source, DeploySource::Git { .. }) {
        return Err(Error::invalid(format!(
            "service '{}' is not deployed from git: a commit cannot be selected",
            svc.name
        )));
    }
    Ok(source)
}

async fn latest_archive(inner: &Inner, svc: &Service) -> Result<DeploySource> {
    let deploys = inner.store.list_deploys(&svc.id, ARCHIVE_SCAN_LIMIT).await?;
    let Some(path) = deploys.iter().find_map(|d| match &d.source {
        DeploySource::Archive { path } => Some(path.clone()),
        _ => None,
    }) else {
        return Err(Error::invalid(format!(
            "service '{}' has no repository or image: upload code with `ferry up {}`",
            svc.name, svc.name
        )));
    };
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(Error::invalid(format!(
            "the last uploaded source of '{}' is no longer on the server: upload code again with `ferry up {}`",
            svc.name, svc.name
        )));
    }
    Ok(DeploySource::Archive { path })
}

/// Insert a queued deploy, supersede older queued ones, wake the worker.
/// With `fold`, an already queued deploy of the service is returned instead
/// (checked under the same lock as the insert, so nothing slips in between).
async fn enqueue(
    inner: &Arc<Inner>,
    svc: &Service,
    trigger: DeployTrigger,
    source: DeploySource,
    opts: DeployOptions,
    fold: bool,
) -> Result<Deploy> {
    let _queue = inner.queue_lock.lock().await;
    // Re-checked under the queue lock: `delete_service` sets the flag before
    // it cancels the queue (under this lock), so nothing slips in between.
    if inner.is_service_deleting(&svc.id) {
        return Err(Error::conflict(format!("service '{}' is being deleted", svc.name)));
    }
    // The shutdown sweep fails queued deploys under this lock too.
    if inner.shutdown.is_cancelled() {
        return Err(Error::conflict("the server is shutting down: deploy again once it is back"));
    }
    if fold && let Some(queued) = next_queued(inner, &svc.id).await? {
        info!(service = %svc.name, deploy = %queued.id, "restart folded into the already queued deploy");
        return Ok(queued);
    }
    let deploy = Deploy::new(&svc.id, trigger, source);
    inner.store.create_deploy(&deploy).await?;
    if opts.clear_cache {
        inner.with_rt(|rt| rt.deploy_options.insert(deploy.id.clone(), opts));
    }
    // Followers may attach while the deploy waits in the queue.
    inner.logs.open(LogKind::Deploy, &deploy.id);
    let reason = format!("superseded by {}", deploy.id);
    match inner.store.active_deploys().await {
        Ok(active) => {
            for old in active {
                if old.service_id == svc.id && old.id != deploy.id && old.status == DeployStatus::Queued {
                    cancel_queued(inner, &old, &reason).await;
                }
            }
        }
        Err(e) => warn!(service = %svc.name, "could not supersede older queued deploys: {e}"),
    }
    ensure_route(inner, svc);
    ensure_worker(inner, &svc.id);
    info!(service = %svc.name, deploy = %deploy.id, trigger = %trigger, "deploy queued");
    Ok(deploy)
}

/// A public service gets its (empty) route as soon as it is deployed, so
/// its hostnames answer "no healthy instances yet" (503) instead of
/// "unknown host" (404) until the first deploy is live.
fn ensure_route(inner: &Inner, svc: &Service) {
    if !svc.is_public_http() || svc.suspended {
        return;
    }
    let hosts = inner.config.service_hosts(svc);
    if !hosts.is_empty() && !inner.routes.snapshot().iter().any(|r| r.service_id == svc.id) {
        inner.routes.set_service_routes(&svc.id, &hosts, Vec::new());
    }
}

/// Mark a queued deploy canceled (caller holds the queue lock).
async fn cancel_queued(inner: &Inner, deploy: &Deploy, reason: &str) {
    match inner.store.set_deploy_status(&deploy.id, DeployStatus::Canceled, Some(reason)).await {
        Ok(_) => info!(deploy = %deploy.id, "{reason}"),
        Err(e) => {
            warn!(deploy = %deploy.id, "could not cancel queued deploy: {e}");
            return;
        }
    }
    inner.with_rt(|rt| rt.deploy_options.remove(&deploy.id));
    let log = inner.logs.open(LogKind::Deploy, &deploy.id);
    log.system(format!("==> Deploy canceled: {reason}"));
    log.finish().await;
}

/// Make sure the service has a live worker, and wake it (caller holds the
/// queue lock, so a new worker cannot exit before its handle is recorded).
fn ensure_worker(inner: &Arc<Inner>, service_id: &str) {
    let spawn = inner.with_rt(|rt| {
        if let Some(w) = rt.workers.get(service_id)
            && w.handle.as_ref().is_none_or(|h| !h.is_finished())
        {
            w.wake.notify_one();
            return None;
        }
        let generation = inner.worker_generation.fetch_add(1, Ordering::Relaxed);
        let wake = Arc::new(Notify::new());
        rt.workers.insert(service_id.to_string(), Worker { generation, wake: wake.clone(), handle: None });
        Some((generation, wake))
    });
    if let Some((generation, wake)) = spawn {
        let handle = inner.spawn(worker_loop(inner.clone(), service_id.to_string(), generation, wake));
        inner.with_rt(|rt| {
            if let Some(w) = rt.workers.get_mut(service_id)
                && w.generation == generation
            {
                w.handle = Some(handle);
            }
        });
    }
}

fn wake_worker(inner: &Inner, service_id: &str) {
    inner.with_rt(|rt| {
        if let Some(w) = rt.workers.get(service_id) {
            w.wake.notify_one();
        }
    });
}

fn deregister_worker(inner: &Inner, service_id: &str, generation: u64) {
    inner.with_rt(|rt| {
        if rt.workers.get(service_id).is_some_and(|w| w.generation == generation) {
            rt.workers.remove(service_id);
        }
    });
}

/// Oldest queued deploy of a service.
async fn next_queued(inner: &Inner, service_id: &str) -> Result<Option<Deploy>> {
    Ok(inner
        .store
        .active_deploys()
        .await?
        .into_iter()
        .find(|d| d.service_id == service_id && d.status == DeployStatus::Queued))
}

fn needs_build_slot(source: &DeploySource) -> bool {
    matches!(source, DeploySource::Git { .. } | DeploySource::Archive { .. })
}

enum Next {
    Deploy(Box<Deploy>),
    Retry,
}

/// A deploy a worker took off the queue.
struct Claimed {
    deploy: Deploy,
    active: ActiveDeploy,
    done: watch::Sender<bool>,
    opts: DeployOptions,
}

/// Processes one service's queue in order until it is empty.
async fn worker_loop(inner: Arc<Inner>, service_id: String, generation: u64, wake: Arc<Notify>) {
    let mut announced_wait: Option<String> = None;
    loop {
        let next = {
            let _queue = inner.queue_lock.lock().await;
            if inner.shutdown.is_cancelled() {
                deregister_worker(&inner, &service_id, generation);
                return;
            }
            match next_queued(&inner, &service_id).await {
                Ok(Some(d)) => Next::Deploy(Box::new(d)),
                Ok(None) => {
                    deregister_worker(&inner, &service_id, generation);
                    return;
                }
                Err(e) => {
                    warn!(service = %service_id, "deploy worker cannot read the queue: {e}");
                    Next::Retry
                }
            }
        };
        let next = match next {
            Next::Deploy(d) => *d,
            Next::Retry => {
                tokio::select! {
                    _ = tokio::time::sleep(WORKER_RETRY) => {}
                    _ = inner.shutdown.cancelled() => {}
                }
                continue;
            }
        };

        // A build slot for builds (pulls and reuses don't need one). While
        // waiting, the deploy stays queued and can still be superseded.
        let permit: Option<OwnedSemaphorePermit> = if needs_build_slot(&next.source) {
            match inner.build_slots.clone().try_acquire_owned() {
                Ok(p) => Some(p),
                Err(_) => {
                    if announced_wait.as_deref() != Some(next.id.as_str()) {
                        announced_wait = Some(next.id.clone());
                        // Only if still open: it may have been canceled since the peek.
                        if let Some(log) = inner.logs.get(LogKind::Deploy, &next.id) {
                            log.system(format!(
                                "==> Waiting for a free build slot ({} build(s) run at a time)",
                                inner.config.build_concurrency.max(1)
                            ));
                        }
                    }
                    tokio::select! {
                        p = inner.build_slots.clone().acquire_owned() => p.ok(),
                        _ = wake.notified() => continue,
                        _ = inner.shutdown.cancelled() => continue,
                    }
                }
            }
        } else {
            None
        };

        let claimed = {
            let _queue = inner.queue_lock.lock().await;
            claim(&inner, &service_id, &next.id).await
        };
        match claimed {
            Ok(Some(claimed)) => run_claimed(&inner, claimed, permit).await,
            // Superseded or canceled meanwhile: look again.
            Ok(None) => {}
            // Store trouble: back off (outside the queue lock).
            Err(()) => {
                drop(permit);
                tokio::select! {
                    _ = tokio::time::sleep(WORKER_RETRY) => {}
                    _ = inner.shutdown.cancelled() => {}
                }
            }
        }
    }
}

/// Take `deploy_id` off the queue if it is still the next queued deploy
/// (caller holds the queue lock). `Err` = the store failed.
async fn claim(inner: &Inner, service_id: &str, deploy_id: &str) -> std::result::Result<Option<Claimed>, ()> {
    match next_queued(inner, service_id).await {
        Ok(Some(d)) if d.id == deploy_id => {}
        Ok(_) => return Ok(None),
        Err(e) => {
            warn!(deploy = deploy_id, "cannot claim deploy: {e}");
            return Err(());
        }
    }
    let deploy = match inner.store.set_deploy_status(deploy_id, DeployStatus::Building, None).await {
        Ok(d) => d,
        Err(e) => {
            warn!(deploy = deploy_id, "cannot start deploy: {e}");
            return Err(());
        }
    };
    let (done_tx, done_rx) = watch::channel(false);
    let active = ActiveDeploy::new(service_id, inner.shutdown.child_token(), done_rx);
    let opts = inner.with_rt(|rt| {
        rt.deploys.insert(deploy_id.to_string(), active.clone());
        rt.deploy_options.remove(deploy_id).unwrap_or_default()
    });
    Ok(Some(Claimed { deploy, active, done: done_tx, opts }))
}

/// Run one claimed deploy in its own task, so that a panic fails that deploy
/// instead of killing the worker.
async fn run_claimed(inner: &Arc<Inner>, claimed: Claimed, permit: Option<OwnedSemaphorePermit>) {
    let Claimed { deploy, active, done, opts } = claimed;
    let log = inner.logs.open(LogKind::Deploy, &deploy.id);
    let task = inner.spawn(pipeline::run(inner.clone(), deploy.clone(), active.clone(), log.clone(), opts, permit));
    if let Err(join_err) = task.await {
        error!(deploy = %deploy.id, "deploy task failed: {join_err}");
        pipeline::handle_crash(inner, &deploy, &log, &panic_message(&join_err)).await;
    }
    log.finish().await;
    inner.with_rt(|rt| rt.deploys.remove(&deploy.id));
    let _ = done.send(true);

    // Cleanup (image retention, old uploads) in its own task as well.
    let cleanup = {
        let inner = inner.clone();
        let service_id = deploy.service_id.clone();
        let tasks = inner.tasks.clone();
        tasks.spawn(async move { pipeline::cleanup_after_deploy(&inner, &service_id).await })
    };
    if let Err(e) = cleanup.await {
        error!(deploy = %deploy.id, "post-deploy cleanup failed: {e}");
    }
    // Converge anything that changed meanwhile (scale during the deploy,
    // a recreate deploy that failed after stopping the old instance...).
    inner.wake_reconciler();
}

/// `Engine::cancel_deploy`.
pub(crate) async fn cancel_deploy(inner: &Arc<Inner>, deploy_id: &str) -> Result<Deploy> {
    let active = {
        let _queue = inner.queue_lock.lock().await;
        let d = inner.store.require_deploy(deploy_id).await?;
        if d.status == DeployStatus::Live {
            return Err(too_late(&d.id));
        }
        if d.status.is_terminal() {
            return Err(Error::conflict(format!("deploy {} is already {}", d.id, d.status)));
        }
        match inner.with_rt(|rt| rt.deploys.get(&d.id).cloned()) {
            Some(active) => active,
            None if d.status == DeployStatus::Queued => {
                cancel_queued(inner, &d, "canceled by user").await;
                wake_worker(inner, &d.service_id);
                return inner.store.require_deploy(&d.id).await;
            }
            None => {
                // Building/deploying but no worker owns it (e.g. a stale row).
                let d = inner.store.set_deploy_status(&d.id, DeployStatus::Canceled, Some("canceled by user")).await?;
                let log = inner.logs.open(LogKind::Deploy, &d.id);
                log.system("==> Deploy canceled");
                log.finish().await;
                return Ok(d);
            }
        }
    };
    // Once traffic was switched to the new instances the deploy goes live
    // no matter what: say so right away instead of waiting for it.
    if !active.request_cancel("canceled by user") {
        return Err(too_late(deploy_id));
    }
    if !active.wait_done(CANCEL_WAIT).await {
        warn!(deploy = deploy_id, "deploy did not stop within {}s of being canceled", CANCEL_WAIT.as_secs());
    }
    inner.store.require_deploy(deploy_id).await
}

fn too_late(deploy_id: &str) -> Error {
    Error::conflict(format!(
        "too late to cancel: traffic was already switched to deploy {deploy_id} (roll back to undo it)"
    ))
}

/// Cancel every queued and running deploy of a service (suspend, delete)
/// and wait for the running ones to stop.
pub(crate) async fn cancel_service_deploys(inner: &Arc<Inner>, service_id: &str, reason: &str) -> Result<()> {
    let running = {
        let _queue = inner.queue_lock.lock().await;
        for d in inner.store.active_deploys().await? {
            if d.service_id == service_id
                && d.status == DeployStatus::Queued
                && !inner.with_rt(|rt| rt.deploys.contains_key(&d.id))
            {
                cancel_queued(inner, &d, reason).await;
            }
        }
        inner.active_deploys_of(service_id)
    };
    for a in &running {
        a.request_cancel(reason);
    }
    for a in &running {
        if !a.wait_done(CANCEL_WAIT).await {
            warn!(service = service_id, "a deploy did not stop within {}s", CANCEL_WAIT.as_secs());
        }
    }
    wake_worker(inner, service_id);
    Ok(())
}

/// Boot: deploys and jobs a previous process left unfinished are failed.
pub(crate) async fn recover_interrupted(inner: &Arc<Inner>) -> Result<()> {
    for d in inner.store.active_deploys().await? {
        let status =
            if d.status == DeployStatus::Deploying { DeployStatus::DeployFailed } else { DeployStatus::BuildFailed };
        inner.store.set_deploy_status(&d.id, status, Some(INTERRUPTED)).await?;
        warn!(deploy = %d.id, "deploy {INTERRUPTED}");
        let log = inner.logs.open(LogKind::Deploy, &d.id);
        log.system(pipeline::failure_line(status, INTERRUPTED));
        log.finish().await;
        // Its containers (if any) belong to no live deploy: the reconciler removes them.
    }
    for mut j in inner.store.active_job_runs().await? {
        j.status = JobStatus::Failed;
        j.error = Some(INTERRUPTED.to_string());
        j.finished_at = Some(chrono::Utc::now());
        inner.store.update_job_run(&j).await?;
        warn!(job = %j.id, "job {INTERRUPTED}");
        let log = inner.logs.open(LogKind::Job, &j.id);
        log.system(format!("==> Job failed: {INTERRUPTED}"));
        log.finish().await;
    }
    Ok(())
}

/// Shutdown: deploys still waiting in the queue will not run. Fail them now
/// (as boot recovery would) and finish their logs, so that clients
/// following them see the end instead of waiting forever.
pub(crate) async fn fail_queued_on_shutdown(inner: Arc<Inner>) {
    inner.shutdown.cancelled().await;
    let _queue = inner.queue_lock.lock().await;
    let queued = match inner.store.active_deploys().await {
        Ok(active) => active.into_iter().filter(|d| d.status == DeployStatus::Queued),
        Err(e) => {
            warn!("cannot fail the queued deploys at shutdown: {e}");
            return;
        }
    };
    for d in queued {
        // Claimed ones end through their own pipeline.
        if inner.with_rt(|rt| rt.deploys.contains_key(&d.id)) {
            continue;
        }
        if let Err(e) = inner.store.set_deploy_status(&d.id, DeployStatus::BuildFailed, Some(SHUTDOWN)).await {
            warn!(deploy = %d.id, "cannot fail queued deploy: {e}");
            continue;
        }
        inner.with_rt(|rt| rt.deploy_options.remove(&d.id));
        info!(deploy = %d.id, "queued deploy {SHUTDOWN}");
        let log = inner.logs.open(LogKind::Deploy, &d.id);
        log.system(pipeline::failure_line(DeployStatus::BuildFailed, SHUTDOWN));
        log.finish().await;
    }
}

/// Remove every container of a deploy right away (failure cleanup).
pub(crate) async fn remove_deploy_containers(inner: &Inner, deploy_id: &str) {
    let prefix = inner.naming.prefix().to_string();
    match inner.docker.list_containers(&[(LABEL_INSTANCE, prefix.as_str()), (LABEL_DEPLOY, deploy_id)], true).await {
        Ok(list) => {
            if let Err(e) = crate::instances::retire(inner, &list, 0).await {
                warn!(deploy = deploy_id, "could not remove every container of the deploy: {}", error_message(&e));
            }
        }
        Err(e) => warn!(deploy = deploy_id, "could not list the deploy's containers: {e}"),
    }
}
