//! `/api/v1/services` — CRUD and lifecycle actions.

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use ferry_core::dto::{CreateService, RollbackRequest, RuntimeStatus, ScaleRequest, ServiceView, UpdateService};
use ferry_core::{
    Deploy, DeployRequest, DeployTrigger, EnvGroup, Error, LogOptions, Runtime, Service, ServiceType, validate,
};
use http::StatusCode;
use serde::Deserialize;

use crate::AppState;
use crate::checks;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath, ApiQuery, de_flag};
use crate::ops;
use crate::sse;
use crate::views::service_view;

/// `GET /api/v1/services`
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<ServiceView>>> {
    let mut out = Vec::new();
    for svc in st.store.list_services().await? {
        out.push(service_view(&st.store, &st.config, svc).await?);
    }
    Ok(Json(out))
}

/// `GET /api/v1/services/{id}`
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// Normalize, validate and check the custom domains of a service row.
fn validated_domains(domains: &[String]) -> Result<Vec<String>, Error> {
    domains.iter().filter(|d| !d.trim().is_empty()).map(|d| validate::domain(d)).collect()
}

/// Build (and validate) the row of a new service from a create request.
fn new_service(req: &CreateService) -> Result<Service, Error> {
    let service_type = req.service_type.unwrap_or(ServiceType::WebService);
    let mut svc = Service::new(req.name.trim(), service_type);
    let has_image = req.image.as_deref().is_some_and(|i| !i.trim().is_empty());
    svc.repo_url = req.repo_url.clone();
    if let Some(b) = &req.branch {
        svc.branch = b.clone();
    }
    svc.image = req.image.clone();
    svc.runtime = req.runtime.unwrap_or(if has_image { Runtime::Image } else { Runtime::Auto });
    svc.root_dir = req.root_dir.clone();
    svc.dockerfile_path = req.dockerfile_path.clone();
    svc.build_command = req.build_command.clone();
    svc.start_command = req.start_command.clone();
    svc.publish_dir = req.publish_dir.clone();
    svc.port = req.port.filter(|p| *p != 0);
    svc.health_check_path = req.health_check_path.clone();
    svc.schedule = req.schedule.clone();
    svc.instances = req.instances.unwrap_or(1);
    svc.auto_deploy = req.auto_deploy.unwrap_or(true);
    svc.disk_mount_path = req.disk_mount_path.clone();
    svc.custom_domains = validated_domains(req.custom_domains.as_deref().unwrap_or_default())?;
    validate::normalize_service(&mut svc);
    validate::service(&svc)?;
    Ok(svc)
}

/// `POST /api/v1/services`
pub async fn create(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<CreateService>,
) -> ApiResult<(StatusCode, Json<ServiceView>)> {
    let svc = new_service(&req)?;
    let env = req.env.clone().unwrap_or_default();
    checks::validate_env_vars(&env)?;
    let mut groups: Vec<EnvGroup> = Vec::new();
    for g in req.env_groups.iter().flatten().map(|g| g.trim()).filter(|g| !g.is_empty()) {
        let group =
            st.store.find_env_group(g).await?.ok_or_else(|| Error::invalid(format!("env group '{g}' not found")))?;
        if !groups.iter().any(|x| x.id == group.id) {
            groups.push(group);
        }
    }
    let existing = st.store.list_services().await?;
    checks::check_default_host_free(&st.config, &existing, &svc.name)?;
    checks::check_domains(&st.config, &existing, &svc.name, &svc.custom_domains)?;

    st.store.create_service(&svc).await?;
    let extras = async {
        if !env.is_empty() {
            st.store.replace_env(&svc.id, &env).await?;
        }
        for g in &groups {
            st.store.link_env_group(&svc.id, &g.id).await?;
        }
        Ok::<(), Error>(())
    }
    .await;
    if let Err(e) = extras {
        // Don't leave a half-configured service behind.
        if let Err(del) = st.store.delete_service(&svc.id).await {
            tracing::error!(service = %svc.name, "rolling back service creation failed: {del}");
        }
        return Err(e.into());
    }
    tracing::info!(service = %svc.name, id = %svc.id, "created service");

    let has_source = svc.repo_url.is_some() || svc.image.is_some();
    if req.deploy.unwrap_or(true) && has_source {
        // The service exists either way; a failure to enqueue is reported in
        // the logs and visible as "not deployed" in the view.
        if let Err(e) = st.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Create)).await {
            tracing::warn!(service = %svc.name, "queuing the first deploy failed: {e}");
        }
    }
    let svc = st.store.require_service(&svc.id).await?;
    Ok((StatusCode::CREATED, Json(service_view(&st.store, &st.config, svc).await?)))
}

/// Apply the provided fields of an update request to `next`.
fn apply_update(next: &mut Service, req: &UpdateService) -> Result<(), Error> {
    fn set(target: &mut Option<String>, value: &Option<String>) {
        if let Some(v) = value {
            // Empty strings clear; `normalize_service` turns "" into None.
            *target = Some(v.clone());
        }
    }
    let non_empty = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.trim().is_empty());
    if let Some(r) = &req.repo_url {
        next.repo_url = Some(r.clone());
        // Switching to a git source drops the image unless the request sets both.
        if non_empty(&req.repo_url) && req.image.is_none() {
            next.image = None;
        }
    }
    if let Some(i) = &req.image {
        next.image = Some(i.clone());
        if non_empty(&req.image) && req.repo_url.is_none() {
            next.repo_url = None;
        }
    }
    match req.runtime {
        Some(r) => next.runtime = r,
        None => {
            let has_image = next.image.as_deref().is_some_and(|i| !i.trim().is_empty());
            if !has_image && next.runtime == Runtime::Image {
                next.runtime = Runtime::Auto;
            }
        }
    }
    if let Some(b) = &req.branch {
        next.branch = b.clone();
    }
    set(&mut next.root_dir, &req.root_dir);
    set(&mut next.dockerfile_path, &req.dockerfile_path);
    set(&mut next.build_command, &req.build_command);
    set(&mut next.start_command, &req.start_command);
    set(&mut next.publish_dir, &req.publish_dir);
    set(&mut next.health_check_path, &req.health_check_path);
    set(&mut next.schedule, &req.schedule);
    set(&mut next.disk_mount_path, &req.disk_mount_path);
    if let Some(p) = req.port {
        next.port = (p != 0).then_some(p);
    }
    if let Some(n) = req.instances {
        next.instances = n;
    }
    if let Some(a) = req.auto_deploy {
        next.auto_deploy = a;
    }
    if let Some(s) = req.suspended {
        next.suspended = s;
    }
    if let Some(d) = &req.custom_domains {
        next.custom_domains = validated_domains(d)?;
    }
    Ok(())
}

/// Stored fields differ (ignoring `updated_at`).
fn row_changed(a: &Service, b: &Service) -> bool {
    let mut a = a.clone();
    a.updated_at = b.updated_at;
    &a != b
}

/// `PATCH /api/v1/services/{id}`
pub async fn update(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateService>,
) -> ApiResult<Json<ServiceView>> {
    let current = st.store.require_service(&id).await?;
    let mut next = current.clone();
    apply_update(&mut next, &req)?;
    validate::normalize_service(&mut next);
    validate::service(&next)?;
    let domains_changed = next.custom_domains != current.custom_domains;
    if domains_changed {
        let all = st.store.list_services().await?;
        checks::check_domains(&st.config, all.iter().filter(|s| s.id != current.id), &next.name, &next.custom_domains)?;
    }

    // Instances and suspension are owned by the engine (scale / suspend);
    // live deploy and hook key are never changed here. Take them from the
    // freshest row so a concurrent deploy isn't overwritten.
    let fresh = st.store.require_service(&current.id).await?;
    let mut row = next.clone();
    row.instances = fresh.instances;
    row.suspended = fresh.suspended;
    row.live_deploy_id = fresh.live_deploy_id.clone();
    row.deploy_hook_key = fresh.deploy_hook_key.clone();
    if row_changed(&row, &fresh) {
        st.store.update_service(&row).await?;
        tracing::info!(service = %row.name, "updated service settings");
    }
    if let Some(n) = req.instances
        && n != fresh.instances
    {
        st.engine.scale(&fresh.id, n).await?;
    }
    if let Some(s) = req.suspended
        && s != fresh.suspended
    {
        if s {
            st.engine.suspend(&fresh.id).await?;
        } else {
            st.engine.resume(&fresh.id).await?;
        }
    }
    if domains_changed {
        ops::refresh_routes_logged(st.engine.as_ref(), &row).await;
    }
    let svc = st.store.require_service(&fresh.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `DELETE /api/v1/services/{id}`
pub async fn delete(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<StatusCode> {
    let svc = st.store.require_service(&id).await?;
    st.engine.delete_service(&svc.id).await?;
    tracing::info!(service = %svc.name, "deleted service");
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/services/{id}/status`
pub async fn status(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<RuntimeStatus>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(st.engine.service_status(&svc.id).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct LogsQuery {
    #[serde(default, deserialize_with = "de_flag")]
    pub follow: bool,
    pub tail: Option<usize>,
}

/// `GET /api/v1/services/{id}/logs?follow=&tail=` (SSE)
pub async fn logs(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<LogsQuery>,
) -> ApiResult<Response> {
    let svc = st.store.require_service(&id).await?;
    let stream = st.engine.service_logs(&svc.id, LogOptions { follow: q.follow, tail: q.tail }).await?;
    Ok(sse::log_response(stream, !q.follow))
}

/// `POST /api/v1/services/{id}/restart`
pub async fn restart(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.require_service(&id).await?;
    let d = st.engine.restart(&svc.id, DeployTrigger::Restart).await?;
    Ok((StatusCode::ACCEPTED, Json(d)))
}

/// `POST /api/v1/services/{id}/suspend`
pub async fn suspend(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    st.engine.suspend(&svc.id).await?;
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `POST /api/v1/services/{id}/resume`
pub async fn resume(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    st.engine.resume(&svc.id).await?;
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `POST /api/v1/services/{id}/scale`
pub async fn scale(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<ScaleRequest>,
) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    let mut check = svc.clone();
    check.instances = req.instances;
    validate::service(&check)?;
    st.engine.scale(&svc.id, req.instances).await?;
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `POST /api/v1/services/{id}/rollback`
pub async fn rollback(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<RollbackRequest>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.require_service(&id).await?;
    let target = st.store.require_deploy(req.deploy_id.trim()).await?;
    if target.service_id != svc.id {
        return Err(Error::invalid(format!("deploy '{}' does not belong to service '{}'", target.id, svc.name)).into());
    }
    let d = st.engine.rollback(&svc.id, &target.id).await?;
    Ok((StatusCode::ACCEPTED, Json(d)))
}
