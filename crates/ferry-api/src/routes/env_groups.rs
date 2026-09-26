//! `/api/v1/env-groups`.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{CreateEnvGroup, EnvGroupView, PatchEnv, ReplaceEnv};
use ferry_core::{EnvGroup, EnvVar, Error, validate};
use http::StatusCode;

use crate::AppState;
use crate::checks::{self, EnvChange};
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath, ApiQuery, DeleteQuery, RestartQuery};
use crate::locks;
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
    locks::detached(async move {
        let name = req.name.trim();
        validate::env_group_name(name)?;
        validate::env_vars(&req.vars)?;
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
    })
    .await
}

/// `GET /api/v1/env-groups/{id}`
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<EnvGroupView>> {
    let g = st.store.require_env_group(&id).await?;
    Ok(Json(env_group_view(&st.store, g).await?))
}

/// `DELETE /api/v1/env-groups/{id}?force=&restart=`
///
/// Refused (409) while services are linked to the group: they would silently
/// lose its variables at their next restart. With `force=true` the group is
/// deleted anyway, and `restart=true` restarts the linked live services so
/// they drop the variables now.
pub async fn delete(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<DeleteQuery>,
) -> ApiResult<StatusCode> {
    locks::detached(async move {
        let g = st.store.require_env_group(&id).await?;
        let _guard = locks::owner(&g.id).await;
        let linked = st.store.env_group_services(&g.id).await?;
        if !linked.is_empty() && !q.force {
            let names: Vec<String> = linked.iter().map(|s| format!("'{}'", s.name)).collect();
            return Err(Error::conflict(format!(
                "env group '{}' is linked to service(s) {}; they would lose its variables at their next restart. Unlink it first, or delete with force=true (and restart=true to restart them now)",
                g.name,
                names.join(", ")
            ))
            .into());
        }
        st.store.delete_env_group(&g.id).await?;
        tracing::info!(group = %g.name, linked = linked.len(), "deleted env group");
        if q.restart {
            ops::restart_services(&st.store, st.engine.as_ref(), &linked).await;
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// A change of an env group's variables.
enum Change {
    Replace(Vec<EnvVar>),
    Patch { set: Vec<EnvVar>, unset: Vec<String> },
}

/// Validate the group's new variables, alone and in the combined env of
/// every linked service; then store them and restart if asked.
async fn change_env(st: AppState, id: String, restart: bool, change: Change) -> ApiResult<Json<EnvGroupView>> {
    let g = st.store.require_env_group(&id).await?;
    let _guard = locks::owner(&g.id).await;
    let next = match &change {
        Change::Replace(vars) => vars.clone(),
        Change::Patch { set, unset } => checks::patched_env(&st.store.list_env(&g.id).await?, set, unset),
    };
    validate::env_vars(&next)?;
    for svc in st.store.env_group_services(&g.id).await? {
        let env_change = EnvChange { group: Some((g.id.as_str(), next.as_slice())), ..Default::default() };
        checks::check_service_env(&st.store, &svc, env_change).await?;
    }
    match &change {
        Change::Replace(vars) => st.store.replace_env(&g.id, vars).await?,
        Change::Patch { set, unset } => st.store.patch_env(&g.id, set, unset).await?,
    }
    st.store.touch_env_group(&g.id).await?;
    tracing::info!(group = %g.name, vars = next.len(), "changed env group vars");
    if restart {
        ops::restart_group_services(&st.store, st.engine.as_ref(), &g.id).await?;
    }
    let g = st.store.require_env_group(&g.id).await?;
    Ok(Json(env_group_view(&st.store, g).await?))
}

/// `PUT /api/v1/env-groups/{id}/env?restart=`
pub async fn replace_env(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<ReplaceEnv>,
) -> ApiResult<Json<EnvGroupView>> {
    locks::detached(change_env(st, id, q.restart, Change::Replace(req.vars))).await
}

/// `PATCH /api/v1/env-groups/{id}/env?restart=`
pub async fn patch_env(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<RestartQuery>,
    ApiJson(req): ApiJson<PatchEnv>,
) -> ApiResult<Json<EnvGroupView>> {
    // Duplicate keys / oversized values in the request itself.
    validate::env_vars(&req.set)?;
    locks::detached(change_env(st, id, q.restart, Change::Patch { set: req.set, unset: req.unset })).await
}
