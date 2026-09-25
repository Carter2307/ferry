//! `/api/v1/env-groups`.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{CreateEnvGroup, EnvGroupView, PatchEnv, ReplaceEnv};
use ferry_core::{EnvGroup, validate};
use http::StatusCode;

use crate::AppState;
use crate::checks;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath, ApiQuery, RestartQuery};
use crate::ops;
use crate::views::env_group_view;

/// `GET /api/v1/env-groups`
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<EnvGroupView>>> {
    let mut out = Vec::new();
    for g in st.store.list_env_groups().await? {
        out.push(env_group_view(&st.store, g).await?);
    }
    Ok(Json(out))
}

/// `POST /api/v1/env-groups`
pub async fn create(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<CreateEnvGroup>,
) -> ApiResult<(StatusCode, Json<EnvGroupView>)> {
    let name = req.name.trim();
    validate::env_group_name(name)?;
    checks::validate_env_vars(&req.vars)?;
    let group = EnvGroup::new(name);
    st.store.create_env_group(&group).await?;
    if !req.vars.is_empty()
        && let Err(e) = st.store.replace_env(&group.id, &req.vars).await
    {
        if let Err(del) = st.store.delete_env_group(&group.id).await {
            tracing::error!(group = %group.name, "rolling back env group creation failed: {del}");
        }
        return Err(e.into());
    }
    tracing::info!(group = %group.name, "created env group");
    Ok((StatusCode::CREATED, Json(env_group_view(&st.store, group).await?)))
}

/// `GET /api/v1/env-groups/{id}`
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<EnvGroupView>> {
    let g = st.store.require_env_group(&id).await?;
    Ok(Json(env_group_view(&st.store, g).await?))
}

/// `DELETE /api/v1/env-groups/{id}`
pub async fn delete(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<StatusCode> {
    let g = st.store.require_env_group(&id).await?;
    st.store.delete_env_group(&g.id).await?;
    tracing::info!(group = %g.name, "deleted env group");
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/env-groups/{id}/env?restart=`
pub async fn replace_env(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<ReplaceEnv>,
) -> ApiResult<Json<EnvGroupView>> {
    let g = st.store.require_env_group(&id).await?;
    checks::validate_env_vars(&req.vars)?;
    st.store.replace_env(&g.id, &req.vars).await?;
    st.store.touch_env_group(&g.id).await?;
    tracing::info!(group = %g.name, vars = req.vars.len(), "replaced env group vars");
    if q.restart {
        ops::restart_group_services(&st.store, st.engine.as_ref(), &g.id).await?;
    }
    let g = st.store.require_env_group(&g.id).await?;
    Ok(Json(env_group_view(&st.store, g).await?))
}

/// `PATCH /api/v1/env-groups/{id}/env?restart=`
pub async fn patch_env(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<PatchEnv>,
) -> ApiResult<Json<EnvGroupView>> {
    let g = st.store.require_env_group(&id).await?;
    checks::validate_env_vars(&req.set)?;
    st.store.patch_env(&g.id, &req.set, &req.unset).await?;
    st.store.touch_env_group(&g.id).await?;
    tracing::info!(group = %g.name, set = req.set.len(), unset = req.unset.len(), "patched env group vars");
    if q.restart {
        ops::restart_group_services(&st.store, st.engine.as_ref(), &g.id).await?;
    }
    let g = st.store.require_env_group(&g.id).await?;
    Ok(Json(env_group_view(&st.store, g).await?))
}
