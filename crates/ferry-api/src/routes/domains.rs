//! Custom domains of a service.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{ApiErrorBody, DomainRequest};
use ferry_core::{Error, validate};

use crate::AppState;
use crate::checks;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath};
use crate::locks;
use crate::ops;

/// `GET /api/v1/services/{id}/domains`
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}/domains",
    tag = "domains",
    operation_id = "listDomains",
    summary = "List custom domains",
    params(("id" = String, Path, description = "Service id or name.")),
    responses(
        (status = 200, description = "The service's custom domains.", body = [String]),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn list(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<Vec<String>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(svc.custom_domains))
}

/// `POST /api/v1/services/{id}/domains`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/domains",
    tag = "domains",
    operation_id = "addDomain",
    summary = "Add a custom domain",
    description = "Web services and static sites only. The domain is normalized (lowercase, no trailing dot) and must not be used by another service or be a default host.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = DomainRequest,
    responses(
        (status = 200, description = "The service's custom domains.", body = [String]),
        (status = 400, description = "Invalid domain, or the service type has no public HTTP.", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "The domain is already added, or another service uses it.", body = ApiErrorBody),
    ),
)]
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
#[utoipa::path(
    delete,
    path = "/api/v1/services/{id}/domains/{domain}",
    tag = "domains",
    operation_id = "removeDomain",
    summary = "Remove a custom domain",
    params(("id" = String, Path, description = "Service id or name."), ("domain" = String, Path, description = "The custom domain.")),
    responses(
        (status = 200, description = "The service's remaining custom domains.", body = [String]),
        (status = 404, description = "No such service, or the domain isn't one of its custom domains.", body = ApiErrorBody),
    ),
)]
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
