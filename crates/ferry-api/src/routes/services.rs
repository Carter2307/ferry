//! `/api/v1/services` — CRUD and lifecycle actions.

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use ferry_core::dto::{
    ApiErrorBody, CreateService, RollbackRequest, RuntimeStatus, ScaleRequest, ServiceView, UpdateService,
};
use ferry_core::{
    Deploy, DeployRequest, DeployTrigger, EnvGroup, Error, LogLine, LogOptions, Runtime, Service, ServiceType, git,
    ids, validate,
};
use http::StatusCode;
use serde::Deserialize;

use crate::AppState;
use crate::checks::{self, RefKind};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, DeleteQuery, de_flag};
use crate::locks;
use crate::ops;
use crate::runtime;
use crate::sse;
use crate::views::service_view;

/// `GET /api/v1/services`
///
/// The state of live services reflects their running instances (`degraded`
/// when some are down), from counts refreshed at most every few seconds
/// (see [`runtime`]).
#[utoipa::path(
    get,
    path = "/api/v1/services",
    tag = "services",
    operation_id = "listServices",
    summary = "List services",
    description = "Every service with its computed fields. The `state` of live services reflects their running instances (`degraded` when some are down), from counts refreshed at most every few seconds.",
    responses((status = 200, description = "All services.", body = [ServiceView])),
)]
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<ServiceView>>> {
    let services = st.store.list_services().await?;
    runtime::refresh(&st.engine, &services).await;
    let mut out = Vec::with_capacity(services.len());
    for svc in services {
        out.push(service_view(&st.store, &st.config, svc).await?);
    }
    Ok(Json(out))
}

/// `GET /api/v1/services/{id}` (state as in [`list`])
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}",
    tag = "services",
    operation_id = "getService",
    summary = "Get a service",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "The service.", body = ServiceView),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    runtime::refresh(&st.engine, std::slice::from_ref(&svc)).await;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// Normalize, validate and check the custom domains of a service row.
fn validated_domains(domains: &[String]) -> Result<Vec<String>, Error> {
    domains.iter().filter(|d| !d.trim().is_empty()).map(|d| validate::domain(d)).collect()
}

/// Cron jobs have no long-running instances (each run starts one container),
/// so an instance count other than 1 is meaningless.
fn check_cron_instances(service_type: ServiceType, instances: Option<u32>) -> Result<(), Error> {
    if service_type == ServiceType::CronJob && instances.is_some_and(|n| n != 1) {
        return Err(Error::invalid(
            "cron jobs have no long-running instances (each run starts one container), so they can't be scaled",
        ));
    }
    Ok(())
}

/// Build (and validate) the row of a new service from a create request.
fn new_service(req: &CreateService) -> Result<Service, Error> {
    let service_type = req.service_type.unwrap_or(ServiceType::WebService);
    check_cron_instances(service_type, req.instances)?;
    let mut svc = Service::new(req.name.trim(), service_type);
    let has_image = req.image.as_deref().is_some_and(|i| !i.trim().is_empty());
    svc.repo_url = req.repo_url.clone();
    svc.git_connection_id = req.git_connection_id.clone();
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
    // 0 = the server default, as in PATCH.
    svc.memory_limit_mb = req.memory_limit_mb.filter(|m| *m != 0);
    svc.cpu_limit = req.cpu_limit.filter(|c| *c != 0.0);
    svc.custom_domains = validated_domains(req.custom_domains.as_deref().unwrap_or_default())?;
    validate::normalize_service(&mut svc);
    validate::service(&svc)?;
    checks::source_paths(&svc)?;
    Ok(svc)
}

/// `POST /api/v1/services`
#[utoipa::path(
    post,
    path = "/api/v1/services",
    tag = "services",
    operation_id = "createService",
    summary = "Create a service",
    description = "Creates the service with its own variables and env group links, then queues a first deploy (trigger `create`) when it has a repository or an image, unless `deploy` is `false`. Services and datastores share one namespace of names. `git_connection_id` names the git connection (a connected GitHub / GitLab account, see the `git` operations) whose token clones `repo_url`, which must then be an http(s) repository on that connection's provider instance. `memory_limit_mb` (MiB, 16 MiB to 1 TiB) and `cpu_limit` (CPUs, 0.01 to 512, rounded to 0.01) limit every instance and job run of the service; omitted or `0` = the server default (see `GET /api/v1/info`).",
    request_body = CreateService,
    responses(
        (status = 201, description = "The new service.", body = ServiceView),
        (status = 400, description = "Invalid settings (including resource limits out of range), name, variables or domains, an unknown env group, or a git connection that is unknown or not for the repository.", body = ApiErrorBody),
        (status = 409, description = "The name, its default host or a custom domain is already taken.", body = ApiErrorBody),
    ),
)]
pub async fn create(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<CreateService>,
) -> ApiResult<(StatusCode, Json<ServiceView>)> {
    // Row, env, links and first deploy: finish even if the client goes away.
    locks::detached(create_service(st, req)).await
}

async fn create_service(st: AppState, req: CreateService) -> ApiResult<(StatusCode, Json<ServiceView>)> {
    let svc = new_service(&req)?;
    checks::git_connection(&st.store, &svc).await?;
    let env = req.env.clone().unwrap_or_default();
    validate::env_vars(&env)?;
    let mut groups: Vec<EnvGroup> = Vec::new();
    for g in req.env_groups.iter().flatten().map(|g| g.trim()).filter(|g| !g.is_empty()) {
        let group =
            st.store.find_env_group(g).await?.ok_or_else(|| Error::invalid(format!("env group '{g}' not found")))?;
        if !groups.iter().any(|x| x.id == group.id) {
            groups.push(group);
        }
    }
    let mut layers = Vec::with_capacity(groups.len() + 1);
    for g in &groups {
        layers.push(st.store.list_env(&g.id).await?);
    }
    layers.push(env.clone());
    checks::effective_env_size(&svc.name, &layers)?;

    {
        // Name (shared with datastores) and host uniqueness are
        // check-then-write across tables / services.
        let _names = locks::names().await;
        let _domains = locks::domains().await;
        let existing = st.store.list_services().await?;
        checks::check_default_host_free(&st.config, &existing, &svc.name)?;
        checks::check_domains(&st.config, &existing, &svc.name, &svc.custom_domains)?;
        st.store.create_service(&svc).await?;
    }
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
    } else if svc.is_public_http() {
        // No deploy yet: route the hosts right away so they answer 503
        // ("no instances yet") instead of 404 until the next reconcile pass.
        if let Err(e) = st.engine.refresh_routes(&svc.id).await {
            tracing::debug!(service = %svc.name, "installing routes for a new service failed: {e}");
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
    // Switching sources must clear the old one explicitly (see check_source_switch).
    let previous_repo = next.repo_url.clone();
    set(&mut next.repo_url, &req.repo_url);
    set(&mut next.image, &req.image);
    match &req.git_connection_id {
        Some(id) => next.git_connection_id = Some(id.clone()),
        // Not mentioned: the connection follows the repository for as long
        // as it stays on the connection's host (its token is for nothing else).
        None => {
            let same_host = match (previous_repo.as_deref(), next.repo_url.as_deref()) {
                (Some(old), Some(new)) => old == new || git::same_http_origin(old, new),
                _ => false,
            };
            if !same_host {
                next.git_connection_id = None;
            }
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
    // Limits: 0 clears (server default). Saved only: they apply with the
    // next deploy or restart, which captures them in its launch spec.
    if let Some(m) = req.memory_limit_mb {
        next.memory_limit_mb = (m != 0).then_some(m);
    }
    if let Some(c) = req.cpu_limit {
        next.cpu_limit = (c != 0.0).then_some(c);
    }
    if let Some(d) = &req.custom_domains {
        next.custom_domains = validated_domains(d)?;
    }
    Ok(())
}

/// Human-readable source of a service (credentials redacted).
fn describe_source(s: &Service) -> String {
    match (&s.image, &s.repo_url) {
        (Some(i), _) => format!("image {i}"),
        (None, Some(r)) => format!("git {}", git::redact_url(r)),
        (None, None) => "uploads".to_string(),
    }
}

/// A service has one source: setting an image on a git service (or a repo on
/// an image service) replaces it, which forgets the stored repository / image.
/// That must be asked for explicitly — by clearing the old source (`""`) in
/// the same request — instead of happening silently as a side effect.
fn check_source_switch(current: &Service, req: &UpdateService) -> Result<(), Error> {
    let sets = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.trim().is_empty());
    let name = &current.name;
    if sets(&req.image)
        && req.repo_url.is_none()
        && let Some(repo) = &current.repo_url
    {
        let image = req.image.as_deref().unwrap_or_default().trim();
        return Err(Error::conflict(format!(
            "service '{name}' deploys from the git repository {}; an image would replace that source and forget the repository. To switch, clear repo_url in the same request (ferry update {name} --image {image} --repo \"\")",
            git::redact_url(repo)
        )));
    }
    if sets(&req.repo_url)
        && req.image.is_none()
        && let Some(image) = &current.image
    {
        return Err(Error::conflict(format!(
            "service '{name}' deploys the image {image}; a repository would replace that source and forget the image. To switch, clear image in the same request (ferry update {name} --repo <url> --image \"\")"
        )));
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
#[utoipa::path(
    patch,
    path = "/api/v1/services/{id}",
    tag = "services",
    operation_id = "updateService",
    summary = "Update a service",
    description = "Every field is optional; for optional string settings an empty string clears the value. `instances` scales, `suspended` suspends or resumes, `custom_domains` refreshes the routes; build settings take effect on the next deploy. Resource limits (`memory_limit_mb`, `cpu_limit`; `0` = back to the server default) are only saved: they take effect with the next deploy or restart, this request doesn't redeploy. Switching between a git repository and an image requires clearing the other source in the same request. `git_connection_id` selects the git connection whose token clones the repository (an empty string removes it); when it is omitted, the service keeps its connection as long as `repo_url` stays on the connection's provider instance and loses it otherwise. When a side effect fails after the settings were saved, the error message says so.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = UpdateService,
    responses(
        (status = 200, description = "The updated service.", body = ServiceView),
        (status = 400, description = "Invalid settings (including resource limits out of range, and a git connection that is unknown or not for the repository).", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "A source switch that doesn't clear the old source, or a custom domain another service uses.", body = ApiErrorBody),
    ),
)]
pub async fn update(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateService>,
) -> ApiResult<Json<ServiceView>> {
    locks::detached(update_service(st, id, req)).await
}

async fn update_service(st: AppState, id: String, req: UpdateService) -> ApiResult<Json<ServiceView>> {
    let service_id = st.store.require_service(&id).await?.id;
    // Lock order: domains first (only when they may change), then the row.
    let domains_guard = match req.custom_domains {
        Some(_) => Some(locks::domains().await),
        None => None,
    };
    let _row_guard = locks::owner(&service_id).await;
    // Read under the lock: nobody else changes the row until we're done.
    let current = st.store.require_service(&service_id).await?;
    check_cron_instances(current.service_type, req.instances)?;
    check_source_switch(&current, &req)?;
    let mut next = current.clone();
    apply_update(&mut next, &req)?;
    validate::normalize_service(&mut next);
    validate::service(&next)?;
    checks::source_paths(&next)?;
    if (&next.git_connection_id, &next.repo_url) != (&current.git_connection_id, &current.repo_url) {
        checks::git_connection(&st.store, &next).await?;
    }
    let domains_changed = next.custom_domains != current.custom_domains;
    if domains_changed {
        let all = st.store.list_services().await?;
        checks::check_domains(&st.config, all.iter().filter(|s| s.id != current.id), &next.name, &next.custom_domains)?;
    }

    // Instances and suspension are owned by the engine (scale / suspend);
    // the live deploy and hook key are never changed here.
    let mut row = next.clone();
    row.instances = current.instances;
    row.suspended = current.suspended;
    row.live_deploy_id = current.live_deploy_id.clone();
    row.deploy_hook_key = current.deploy_hook_key.clone();
    let mut applied = false;
    if row_changed(&row, &current) {
        st.store.update_service(&row).await?;
        applied = true;
        let (from, to) = (describe_source(&current), describe_source(&row));
        if from != to {
            tracing::info!(service = %row.name, %from, %to, "switched the service's source");
        }
        tracing::info!(service = %row.name, "updated service settings");
    }
    drop(domains_guard);

    // Side effects. Suspend before scaling (don't start containers that are
    // about to be stopped) and resume after it (start the right count once).
    // Earlier parts of the request are already applied when one fails.
    let failed = |what: String, applied: bool| {
        move |e: Error| {
            let err = ApiError::from(e);
            if applied { err.prefixed(format!("settings saved, but {what} failed: ")) } else { err }
        }
    };
    let name = current.name.clone();
    let scale_to = req.instances.filter(|n| *n != current.instances);
    let suspend = req.suspended.filter(|s| *s != current.suspended);
    if suspend == Some(true) {
        st.engine.suspend(&service_id).await.map_err(failed(format!("suspending '{name}'"), applied))?;
        applied = true;
    }
    if let Some(n) = scale_to {
        st.engine.scale(&service_id, n).await.map_err(failed(format!("scaling '{name}' to {n}"), applied))?;
        applied = true;
    }
    if suspend == Some(false) {
        st.engine.resume(&service_id).await.map_err(failed(format!("resuming '{name}'"), applied))?;
        applied = true;
    }
    if domains_changed {
        ops::refresh_routes_logged(st.engine.as_ref(), &row).await;
    }
    if ops::cron_command_changed(&current, &row) {
        // Cron runs use the command of the live deploy's snapshot: without a
        // restart the new command would never run.
        let fresh = st.store.require_service(&service_id).await?;
        if fresh.suspended && fresh.live_deploy_id.is_some() {
            tracing::info!(service = %name, "cron command changed while suspended: restart after resuming to apply it");
        }
        ops::restart_if_deployed(&st.store, st.engine.as_ref(), &fresh, DeployTrigger::Restart)
            .await
            .map_err(failed(format!("restarting '{name}' to apply the new command"), applied))?;
    }
    let svc = st.store.require_service(&service_id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `DELETE /api/v1/services/{id}?force=`
///
/// Refused (409) while other services reference this one in their env
/// (`${{service.NAME...}}`): they would fail every later deploy / restart.
#[utoipa::path(
    delete,
    path = "/api/v1/services/{id}",
    tag = "services",
    operation_id = "deleteService",
    summary = "Delete a service",
    description = "Removes its containers, routes, deploys and variables. Refused while other services reference it in their env (`${{service.NAME...}}`), unless `force=true`.",
    params(
        ("id" = String, Path, description = "Service id or name."),
        ("force" = Option<bool>, Query, description = "Delete even though other services reference it (they will fail to deploy or restart)."),
    ),
    responses(
        (status = 204, description = "Deleted."),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "Other services reference it (the message lists them).", body = ApiErrorBody),
    ),
)]
pub async fn delete(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<DeleteQuery>,
) -> ApiResult<StatusCode> {
    let svc = st.store.require_service(&id).await?;
    let users = checks::referencing_services(&st.store, RefKind::Service, &svc.name, Some(&svc.id)).await?;
    if !users.is_empty() {
        if !q.force {
            return Err(Error::conflict(format!(
                "service '{}' is referenced by {}; they would fail to deploy or restart without it. Remove the references first, or delete with force=true",
                svc.name,
                users.join(", ")
            ))
            .into());
        }
        tracing::warn!(service = %svc.name, users = %users.join(", "), "deleting a service other services reference");
    }
    st.engine.delete_service(&svc.id).await?;
    tracing::info!(service = %svc.name, "deleted service");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/services/{id}/deploy-hook/rotate` — replace the deploy hook
/// key (the old hook URL stops working).
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/deploy-hook/rotate",
    tag = "services",
    operation_id = "rotateDeployHook",
    summary = "Rotate the deploy hook key",
    description = "Replaces the secret of the service's deploy hook URL: the old URL stops working, the new one is the returned `deploy_hook_path`.",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "The service with its new `deploy_hook_path`.", body = ServiceView),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn rotate_deploy_hook(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<ServiceView>> {
    locks::detached(async move {
        let service_id = st.store.require_service(&id).await?.id;
        let _guard = locks::owner(&service_id).await;
        let mut svc = st.store.require_service(&service_id).await?;
        svc.deploy_hook_key = ids::random_secret(32);
        let svc = st.store.update_service(&svc).await?;
        tracing::info!(service = %svc.name, "rotated the deploy hook key");
        Ok(Json(service_view(&st.store, &st.config, svc).await?))
    })
    .await
}

/// `GET /api/v1/services/{id}/status`
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}/status",
    tag = "services",
    operation_id = "getServiceStatus",
    summary = "Runtime status",
    description = "The live state of each container: Docker state, host port, restarts, CPU and memory.",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "Runtime status.", body = RuntimeStatus),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn status(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<RuntimeStatus>> {
    let svc = st.store.require_service(&id).await?;
    let status = st.engine.service_status(&svc.id).await?;
    runtime::record(&svc, &status);
    Ok(Json(status))
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogsQuery {
    /// Keep the stream open and send new lines as they are written.
    #[serde(default, deserialize_with = "de_flag")]
    pub follow: bool,
    /// Start with at most this many recent lines per container.
    pub tail: Option<usize>,
}

/// `GET /api/v1/services/{id}/logs?follow=&tail=` (SSE)
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}/logs",
    tag = "services",
    operation_id = "streamServiceLogs",
    summary = "Runtime logs (SSE)",
    description = "The merged output of the service's current containers (`instance` names the container). Cron jobs have no long-running containers: their stream is a few `system` lines pointing at the job logs.",
    params(("id" = String, Path, description = "Service id or name."), LogsQuery),
    responses(
        (status = 200, description = crate::openapi::SSE_RUNTIME_LOGS, content_type = "text/event-stream", body = String, example = "event: log\ndata: {\"ts\":\"2026-01-01T12:00:00Z\",\"stream\":\"system\",\"line\":\"==> Build succeeded\"}\n\nevent: end\ndata: \n\n"),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn logs(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<LogsQuery>,
) -> ApiResult<Response> {
    let svc = st.store.require_service(&id).await?;
    if svc.service_type == ServiceType::CronJob {
        // No long-running containers: point at the per-run job logs instead
        // of returning nothing (or, with follow, hanging forever).
        let name = &svc.name;
        let mut lines = vec![
            LogLine::system(format!(
                "==> '{name}' is a cron job: it has no long-running instances, so there are no runtime logs."
            )),
            LogLine::system(format!(
                "==> Each run has its own log: list runs with `ferry jobs {name}`, then `ferry logs --job <job id>`."
            )),
        ];
        if let Some(job) = st.store.list_job_runs(&svc.id, 1).await?.into_iter().next() {
            lines.push(LogLine::system(format!("==> Latest run: {} ({})", job.id, job.status)));
        }
        return Ok(sse::log_response(Box::pin(futures::stream::iter(lines)), true, &st.shutdown));
    }
    let stream = st.engine.service_logs(&svc.id, LogOptions { follow: q.follow, tail: q.tail }).await?;
    Ok(sse::log_response(stream, !q.follow, &st.shutdown))
}

/// `POST /api/v1/services/{id}/restart`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/restart",
    tag = "services",
    operation_id = "restartService",
    summary = "Restart a service",
    description = "Rolls out the live deploy's image again (a new deploy with trigger `restart`), picking up changed variables.",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 202, description = "The restart deploy, queued.", body = Deploy),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "The service has no live deploy yet, or it is suspended or being deleted.", body = ApiErrorBody),
    ),
)]
pub async fn restart(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.require_service(&id).await?;
    let d = st.engine.restart(&svc.id, DeployTrigger::Restart).await?;
    Ok((StatusCode::ACCEPTED, Json(d)))
}

/// `POST /api/v1/services/{id}/suspend`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/suspend",
    tag = "services",
    operation_id = "suspendService",
    summary = "Suspend a service",
    description = "Stops its containers; settings, variables and deploys stay until it is resumed.",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "The suspended service.", body = ServiceView),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn suspend(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    st.engine.suspend(&svc.id).await?;
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `POST /api/v1/services/{id}/resume`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/resume",
    tag = "services",
    operation_id = "resumeService",
    summary = "Resume a service",
    description = "Starts the live deploy's containers again.",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "The resumed service.", body = ServiceView),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn resume(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    st.engine.resume(&svc.id).await?;
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `POST /api/v1/services/{id}/scale`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/scale",
    tag = "services",
    operation_id = "scaleService",
    summary = "Scale a service",
    description = "Sets the number of instances. Cron jobs can't be scaled, and services with a disk run 1 instance.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = ScaleRequest,
    responses(
        (status = 200, description = "The scaled service.", body = ServiceView),
        (status = 400, description = "Invalid instance count for this service.", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn scale(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<ScaleRequest>,
) -> ApiResult<Json<ServiceView>> {
    locks::detached(async move {
        let service_id = st.store.require_service(&id).await?.id;
        // Validated against the row a concurrent PATCH (e.g. adding a disk,
        // which limits the service to 1 instance) can't change meanwhile.
        let _row_guard = locks::owner(&service_id).await;
        let svc = st.store.require_service(&service_id).await?;
        check_cron_instances(svc.service_type, Some(req.instances))?;
        let mut check = svc.clone();
        check.instances = req.instances;
        validate::service(&check)?;
        st.engine.scale(&svc.id, req.instances).await?;
        let svc = st.store.require_service(&svc.id).await?;
        Ok(Json(service_view(&st.store, &st.config, svc).await?))
    })
    .await
}

/// `POST /api/v1/services/{id}/rollback`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/rollback",
    tag = "services",
    operation_id = "rollbackService",
    summary = "Roll back to an earlier deploy",
    description = "Queues a deploy (trigger `rollback`) reusing the image of an earlier deploy of this service that went live.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = RollbackRequest,
    responses(
        (status = 202, description = "The rollback deploy, queued.", body = Deploy),
        (status = 400, description = "The deploy belongs to another service, never went live or has no image.", body = ApiErrorBody),
        (status = 404, description = "No such service or deploy.", body = ApiErrorBody),
        (status = 409, description = "The service is suspended or being deleted.", body = ApiErrorBody),
    ),
)]
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
