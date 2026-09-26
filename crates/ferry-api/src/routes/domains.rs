//! Custom domains of a service.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::DomainRequest;
use ferry_core::{Error, validate};

use crate::AppState;
use crate::checks;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath};
use crate::locks;
use crate::ops;

/// `GET /api/v1/services/{id}/domains`
pub async fn list(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<Vec<String>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(svc.custom_domains))
}

/// `POST /api/v1/services/{id}/domains`
pub async fn add(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<DomainRequest>,
) -> ApiResult<Json<Vec<String>>> {
    locks::detached(async move {
        let service_id = st.store.require_service(&id).await?.id;
        let domain = validate::domain(&req.domain)?;
        // The uniqueness check and the write happen under the domain lock (no
        // other service can claim the host meanwhile), the read-modify-write
        // of the row under its lock (no concurrent add is lost).
        let domains_guard = locks::domains().await;
        let row_guard = locks::owner(&service_id).await;
        let mut svc = st.store.require_service(&service_id).await?;
        if !svc.is_public_http() {
            return Err(Error::invalid("custom domains are only supported on web services and static sites").into());
        }
        if svc.custom_domains.contains(&domain) {
            return Err(Error::conflict(format!("domain '{domain}' is already added to service '{}'", svc.name)).into());
        }
        let all = st.store.list_services().await?;
        checks::check_domains(
            &st.config,
            all.iter().filter(|s| s.id != svc.id),
            &svc.name,
            std::slice::from_ref(&domain),
        )?;
        svc.custom_domains.push(domain.clone());
        let updated = st.store.update_service(&svc).await?;
        drop((row_guard, domains_guard));
        tracing::info!(service = %updated.name, %domain, "added custom domain");
        ops::refresh_routes_logged(st.engine.as_ref(), &updated).await;
        Ok(Json(updated.custom_domains))
    })
    .await
}

/// `DELETE /api/v1/services/{id}/domains/{domain}`
pub async fn remove(
    State(st): State<AppState>,
    ApiPath((id, domain)): ApiPath<(String, String)>,
) -> ApiResult<Json<Vec<String>>> {
    locks::detached(async move {
        let service_id = st.store.require_service(&id).await?.id;
        let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        let row_guard = locks::owner(&service_id).await;
        let mut svc = st.store.require_service(&service_id).await?;
        let before = svc.custom_domains.len();
        svc.custom_domains.retain(|d| !d.eq_ignore_ascii_case(&domain));
        if svc.custom_domains.len() == before {
            return Err(Error::NotFound(format!("domain '{domain}' on service '{}'", svc.name)).into());
        }
        let updated = st.store.update_service(&svc).await?;
        drop(row_guard);
        tracing::info!(service = %updated.name, %domain, "removed custom domain");
        ops::refresh_routes_logged(st.engine.as_ref(), &updated).await;
        Ok(Json(updated.custom_domains))
    })
    .await
}
