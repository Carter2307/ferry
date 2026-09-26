//! Service env vars and env group links.
//!
//! Every change runs under the service's row lock (validation against the
//! current variables, then the write) and on a detached task, so the write
//! and the optional restart can't be separated by a client disconnect.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{ApiErrorBody, LinkEnvGroup, PatchEnv, ReplaceEnv, ServiceView};
use ferry_core::{EnvVar, Error, Service, validate};

use crate::AppState;
use crate::checks::{self, EnvChange};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, RestartQuery};
use crate::locks;
use crate::ops;
use crate::views::service_view;

/// `GET /api/v1/services/{id}/env`
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}/env",
    tag = "env",
    operation_id = "listServiceEnv",
    summary = "List a service's variables",
    description = "The service's own variables (not those of its env groups).",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "The variables.", body = [EnvVar]),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn list(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<Vec<EnvVar>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(st.store.list_env(&svc.id).await?))
}

/// Restart after an env change when asked to and when the service's
/// effective environment really changed since `before` (see
/// [`ops::restart_for_env_change`]). The variables are already saved, so a
/// failed restart is reported as the request's error, saying so.
async fn maybe_restart(st: &AppState, svc: &Service, restart: bool, before: &ops::EnvSnapshot) -> ApiResult<()> {
    if !restart {
        return Ok(());
    }
    match before.changed(&st.store, &svc.id).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!(service = %svc.name, "environment unchanged; not restarting");
            return Ok(());
        }
        Err(e) => tracing::warn!(service = %svc.name, "comparing the environment failed; restarting anyway: {e}"),
    }
    let fresh = st.store.require_service(&svc.id).await?;
    ops::restart_for_env_change(&st.store, st.engine.as_ref(), &fresh)
        .await
        .map_err(|e| ApiError::from(e).prefixed(format!("env vars saved, but restarting '{}' failed: ", svc.name)))?;
    Ok(())
}

/// Resolve `id` and lock the service's row; returns the row read under the lock.
async fn locked_service(st: &AppState, id: &str) -> ApiResult<(Service, tokio::sync::OwnedMutexGuard<()>)> {
    let service_id = st.store.require_service(id).await?.id;
    let guard = locks::owner(&service_id).await;
    Ok((st.store.require_service(&service_id).await?, guard))
}

/// `PUT /api/v1/services/{id}/env?restart=`
#[utoipa::path(
    put,
    path = "/api/v1/services/{id}/env",
    tag = "env",
    operation_id = "replaceServiceEnv",
    summary = "Replace a service's variables",
    description = "The service's own variables become exactly `vars`. They are validated alone and merged with the linked env groups (size limits, `${{...}}` references).",
    params(("id" = String, Path, description = "Service id or name."), RestartQuery),
    request_body = ReplaceEnv,
    responses(
        (status = 200, description = "The service's variables.", body = [EnvVar]),
        (status = 400, description = "Invalid variables (keys, duplicates, sizes, references).", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn replace(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<ReplaceEnv>,
) -> ApiResult<Json<Vec<EnvVar>>> {
    locks::detached(async move {
        let (svc, _guard) = locked_service(&st, &id).await?;
        validate::env_vars(&req.vars)?;
        checks::check_service_env(&st.store, &svc, EnvChange { own: Some(&req.vars), ..Default::default() }).await?;
        let before = ops::EnvSnapshot::capture(&st.store, std::slice::from_ref(&svc.id)).await?;
        if ops::same_env(&st.store.list_env(&svc.id).await?, &req.vars) {
            tracing::info!(service = %svc.name, "env vars unchanged");
        } else {
            st.store.replace_env(&svc.id, &req.vars).await?;
            tracing::info!(service = %svc.name, vars = req.vars.len(), "replaced env vars");
        }
        maybe_restart(&st, &svc, q.restart, &before).await?;
        Ok(Json(st.store.list_env(&svc.id).await?))
    })
    .await
}

/// `PATCH /api/v1/services/{id}/env?restart=`
#[utoipa::path(
    patch,
    path = "/api/v1/services/{id}/env",
    tag = "env",
    operation_id = "patchServiceEnv",
    summary = "Set and unset variables",
    description = "Upserts `set` and deletes `unset`; the other variables stay.",
    params(("id" = String, Path, description = "Service id or name."), RestartQuery),
    request_body = PatchEnv,
    responses(
        (status = 200, description = "The service's variables.", body = [EnvVar]),
        (status = 400, description = "Invalid variables (keys, duplicates, sizes, references).", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn patch(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<PatchEnv>,
) -> ApiResult<Json<Vec<EnvVar>>> {
    locks::detached(async move {
        let (svc, _guard) = locked_service(&st, &id).await?;
        validate::env_vars(&req.set)?;
        // The limits apply to the result, not just to the request.
        let current = st.store.list_env(&svc.id).await?;
        let merged = checks::patched_env(&current, &req.set, &req.unset);
        validate::env_vars(&merged)?;
        checks::check_service_env(&st.store, &svc, EnvChange { own: Some(&merged), ..Default::default() }).await?;
        let before = ops::EnvSnapshot::capture(&st.store, std::slice::from_ref(&svc.id)).await?;
        if ops::same_env(&current, &merged) {
            tracing::info!(service = %svc.name, "env vars unchanged");
        } else {
            st.store.patch_env(&svc.id, &req.set, &req.unset).await?;
            tracing::info!(service = %svc.name, set = req.set.len(), unset = req.unset.len(), "patched env vars");
        }
        maybe_restart(&st, &svc, q.restart, &before).await?;
        Ok(Json(st.store.list_env(&svc.id).await?))
    })
    .await
}

/// `POST /api/v1/services/{id}/env-groups`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/env-groups",
    tag = "env-groups",
    operation_id = "linkEnvGroup",
    summary = "Link an env group to a service",
    description = "The group's variables apply from the service's next deploy or restart (its own variables win). Linking an already linked group is a no-op.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = LinkEnvGroup,
    responses(
        (status = 200, description = "The service with its `env_groups`.", body = ServiceView),
        (status = 400, description = "The combined environment would be invalid (sizes).", body = ApiErrorBody),
        (status = 404, description = "No such service or env group.", body = ApiErrorBody),
    ),
)]
pub async fn link_group(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<LinkEnvGroup>,
) -> ApiResult<Json<ServiceView>> {
    locks::detached(async move {
        let (svc, _guard) = locked_service(&st, &id).await?;
        let group = st.store.require_env_group(req.group.trim()).await?;
        let linked = st.store.service_env_groups(&svc.id).await?;
        if !linked.iter().any(|g| g.id == group.id) {
            let vars = st.store.list_env(&group.id).await?;
            checks::check_service_env(&st.store, &svc, EnvChange { link: Some(&vars), ..Default::default() }).await?;
        }
        st.store.link_env_group(&svc.id, &group.id).await?;
        tracing::info!(service = %svc.name, group = %group.name, "linked env group");
        let svc = st.store.require_service(&svc.id).await?;
        Ok(Json(service_view(&st.store, &st.config, svc).await?))
    })
    .await
}

/// `DELETE /api/v1/services/{id}/env-groups/{group}`
#[utoipa::path(
    delete,
    path = "/api/v1/services/{id}/env-groups/{group}",
    tag = "env-groups",
    operation_id = "unlinkEnvGroup",
    summary = "Unlink an env group from a service",
    params(("id" = String, Path, description = "Service id or name."), ("group" = String, Path, description = "Env group id or name.")),
    responses(
        (status = 200, description = "The service with its `env_groups`.", body = ServiceView),
        (status = 404, description = "No such service or env group, or the group isn't linked to it.", body = ApiErrorBody),
    ),
)]
pub async fn unlink_group(
    State(st): State<AppState>,
    ApiPath((id, group)): ApiPath<(String, String)>,
) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    let group = st.store.require_env_group(&group).await?;
    if !st.store.unlink_env_group(&svc.id, &group.id).await? {
        return Err(Error::NotFound(format!("env group '{}' linked to service '{}'", group.name, svc.name)).into());
    }
    tracing::info!(service = %svc.name, group = %group.name, "unlinked env group");
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}
