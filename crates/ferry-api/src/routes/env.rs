//! Service env vars and env group links.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{LinkEnvGroup, PatchEnv, ReplaceEnv, ServiceView};
use ferry_core::{EnvVar, Error, Service};

use crate::AppState;
use crate::checks;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, RestartQuery};
use crate::ops;
use crate::views::service_view;

/// `GET /api/v1/services/{id}/env`
pub async fn list(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<Vec<EnvVar>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(st.store.list_env(&svc.id).await?))
}

/// Restart after an env change when asked to. The variables are already
/// saved, so a failed restart is reported as the request's error.
async fn maybe_restart(st: &AppState, svc: &Service, restart: bool) -> ApiResult<()> {
    if restart {
        let fresh = st.store.require_service(&svc.id).await?;
        ops::restart_for_env_change(st.engine.as_ref(), &fresh).await.map_err(|e| {
            let mut err = ApiError::from(e);
            err.body.error.message =
                format!("env vars saved, but restarting '{}' failed: {}", svc.name, err.body.error.message);
            err
        })?;
    }
    Ok(())
}

/// `PUT /api/v1/services/{id}/env?restart=`
pub async fn replace(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<ReplaceEnv>,
) -> ApiResult<Json<Vec<EnvVar>>> {
    let svc = st.store.require_service(&id).await?;
    checks::validate_env_vars(&req.vars)?;
    st.store.replace_env(&svc.id, &req.vars).await?;
    tracing::info!(service = %svc.name, vars = req.vars.len(), "replaced env vars");
    maybe_restart(&st, &svc, q.restart).await?;
    Ok(Json(st.store.list_env(&svc.id).await?))
}

/// `PATCH /api/v1/services/{id}/env?restart=`
pub async fn patch(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<PatchEnv>,
) -> ApiResult<Json<Vec<EnvVar>>> {
    let svc = st.store.require_service(&id).await?;
    checks::validate_env_vars(&req.set)?;
    st.store.patch_env(&svc.id, &req.set, &req.unset).await?;
    tracing::info!(service = %svc.name, set = req.set.len(), unset = req.unset.len(), "patched env vars");
    maybe_restart(&st, &svc, q.restart).await?;
    Ok(Json(st.store.list_env(&svc.id).await?))
}

/// `POST /api/v1/services/{id}/env-groups`
pub async fn link_group(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<LinkEnvGroup>,
) -> ApiResult<Json<ServiceView>> {
    let svc = st.store.require_service(&id).await?;
    let group = st.store.require_env_group(req.group.trim()).await?;
    st.store.link_env_group(&svc.id, &group.id).await?;
    tracing::info!(service = %svc.name, group = %group.name, "linked env group");
    let svc = st.store.require_service(&svc.id).await?;
    Ok(Json(service_view(&st.store, &st.config, svc).await?))
}

/// `DELETE /api/v1/services/{id}/env-groups/{group}`
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
