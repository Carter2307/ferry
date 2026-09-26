//! Service operations outside deploys: suspend, resume, scale, delete and
//! route refreshes.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use ferry_core::naming::{LABEL_INSTANCE, LABEL_SERVICE};
use ferry_core::validate::MAX_INSTANCES;
use ferry_core::{DeploySource, Error, Result, ServiceType};
use tracing::{info, warn};

use crate::images::is_own_image;
use crate::instances::{self, STOP_GRACE_SECS};
use crate::logs::LogKind;
use crate::state::{Inner, SetGuard, deleting_services_set};
use crate::{deploy, jobs, pipeline, reconcile};

fn check_not_deleting(inner: &Inner, service_id: &str, name: &str) -> Result<()> {
    if inner.is_service_deleting(service_id) {
        return Err(Error::conflict(format!("service '{name}' is being deleted")));
    }
    Ok(())
}

/// `Engine::suspend`: 503 at the proxy right away, cancel deploys in
/// progress, then stop and remove every instance.
pub(crate) async fn suspend(inner: &Arc<Inner>, service_id: &str) -> Result<()> {
    let svc = inner.store.require_service(service_id).await?;
    check_not_deleting(inner, &svc.id, &svc.name)?;
    inner.store.set_suspended(&svc.id, true).await?;
    let hosts = inner.config.service_hosts(&svc);
    if !hosts.is_empty() {
        inner.routes.set_service_suspended(&svc.id, &hosts);
    }
    deploy::cancel_service_deploys(inner, &svc.id, "service suspended").await?;
    let _lock = inner.service_locks.lock(&svc.id).await;
    let containers = instances::service_containers(inner, &svc.id, true).await?;
    if !containers.is_empty() {
        info!(service = %svc.name, count = containers.len(), "stopping instances of suspended service");
    }
    let result = instances::retire(inner, &containers, STOP_GRACE_SECS).await;
    if !hosts.is_empty() {
        inner.routes.set_service_suspended(&svc.id, &hosts);
    }
    result?;
    info!(service = %svc.name, "service suspended");
    Ok(())
}

/// `Engine::resume`: start the live deploy's instances again (routes follow
/// as soon as they accept connections).
pub(crate) async fn resume(inner: &Arc<Inner>, service_id: &str) -> Result<()> {
    let svc = inner.store.require_service(service_id).await?;
    check_not_deleting(inner, &svc.id, &svc.name)?;
    inner.store.set_suspended(&svc.id, false).await?;
    let _lock = inner.service_locks.lock(&svc.id).await;
    reconcile::converge_service(inner, &svc.id).await?;
    info!(service = %svc.name, "service resumed");
    Ok(())
}

/// `Engine::scale`: persist and converge in place (no new deploy). During
/// a deploy's deploying phase the count is only stored; the reconciler
/// converges right after the deploy.
pub(crate) async fn scale(inner: &Arc<Inner>, service_id: &str, instances: u32) -> Result<()> {
    if instances == 0 {
        return Err(Error::invalid("instances must be at least 1 (use suspend to stop a service)"));
    }
    if instances > MAX_INSTANCES {
        return Err(Error::invalid(format!("instances must be between 1 and {MAX_INSTANCES}")));
    }
    let svc = inner.store.require_service(service_id).await?;
    check_not_deleting(inner, &svc.id, &svc.name)?;
    if svc.disk_mount_path.is_some() && instances > 1 {
        return Err(Error::invalid("services with a disk are limited to 1 instance"));
    }
    inner.store.set_instances(&svc.id, instances).await?;
    info!(service = %svc.name, instances, "scaled service");
    if svc.service_type == ServiceType::CronJob || inner.is_deploying(&svc.id) {
        return Ok(());
    }
    let _lock = inner.service_locks.lock(&svc.id).await;
    reconcile::converge_service(inner, &svc.id).await
}

/// `Engine::refresh_routes`: re-read the hosts. While the service is busy
/// (a deploy owns it), only the hosts change and the upstreams stay.
pub(crate) async fn refresh_routes(inner: &Arc<Inner>, service_id: &str) -> Result<()> {
    let svc = inner.store.require_service(service_id).await?;
    if inner.is_deploying(&svc.id) {
        reconcile::rehost_routes(inner, &svc);
        return Ok(());
    }
    match inner.service_locks.try_lock(&svc.id) {
        Some(_lock) => {
            reconcile::refresh_routes_locked(inner, &svc.id).await?;
        }
        None => reconcile::rehost_routes(inner, &svc),
    }
    Ok(())
}

/// `Engine::delete_service`: cancel deploys and jobs, remove routes,
/// containers, images, disk, git cache, uploads and logs, then the rows.
pub(crate) async fn delete_service(inner: &Arc<Inner>, service_id: &str) -> Result<()> {
    let svc = inner.store.require_service(service_id).await?;
    let Some(_deleting) = SetGuard::insert(inner, &svc.id, deleting_services_set) else {
        return Err(Error::conflict(format!("service '{}' is already being deleted", svc.name)));
    };
    deploy::cancel_service_deploys(inner, &svc.id, "service deleted").await?;
    jobs::stop_service_jobs(inner, &svc.id).await;
    let _lock = inner.service_locks.lock(&svc.id).await;
    inner.routes.remove_service(&svc.id);

    // Containers of the service and its jobs.
    let prefix = inner.naming.prefix().to_string();
    let containers = inner
        .docker
        .list_containers(&[(LABEL_INSTANCE, prefix.as_str()), (LABEL_SERVICE, svc.id.as_str())], true)
        .await?;
    instances::retire(inner, &containers, STOP_GRACE_SECS).await?;

    // Images built for it (never pulled public images).
    let deploys = inner.store.list_deploys(&svc.id, u32::MAX).await?;
    let repo = inner.naming.image_repo(&svc.name);
    let images: HashSet<&str> =
        deploys.iter().filter_map(|d| d.image.as_deref()).filter(|i| is_own_image(i, &repo)).collect();
    for image in images {
        if let Err(e) = inner.docker.remove_image(image).await {
            warn!(service = %svc.name, image, "cannot remove image: {e}");
        }
    }

    instances::remove_volume_retrying(inner, &inner.naming.service_volume(&svc.id)).await?;
    if let Err(e) = inner.builder.remove_repo_cache(&svc.id).await {
        warn!(service = %svc.name, "cannot remove the git cache: {e}");
    }

    let jobs = inner.store.list_job_runs(&svc.id, u32::MAX).await?;
    inner.store.delete_service(&svc.id).await?;

    // Files that belonged to the deleted rows.
    let mut uploads: HashSet<&str> = HashSet::new();
    for d in &deploys {
        if let DeploySource::Archive { path } = &d.source
            && uploads.insert(path.as_str())
        {
            pipeline::remove_upload(inner, Path::new(path)).await;
        }
        inner.logs.remove(LogKind::Deploy, &d.id).await;
    }
    for j in &jobs {
        inner.logs.remove(LogKind::Job, &j.id).await;
    }
    let ids: Vec<String> = deploys.iter().map(|d| d.id.clone()).collect();
    instances::forget_deploys(&inner.store, &ids).await;
    inner.service_locks.forget(&svc.id);
    info!(service = %svc.name, "service deleted");
    Ok(())
}
