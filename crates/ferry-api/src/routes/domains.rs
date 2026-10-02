//! `/api/v1/domains` — the domains services are served under
//! (`<service>.<domain>`, DESIGN.md §21) — and `/api/v1/certificates`.
//!
//! A domain is connected here, then points at the server with one DNS
//! record (`records`); the engine verifies it and serves the services under
//! it as soon as its names reach the server. The server's `--base-domain`
//! is one of the domains: always served, and only the flag removes it.

use std::net::IpAddr;

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{ApiErrorBody, CertificateView, ConnectDomain, DomainView, UpdateDomain};
use ferry_core::{Domain, DomainSource, Error, domains};
use http::StatusCode;

use crate::AppState;
use crate::checks;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath};
use crate::locks;
use crate::views::{certificate_view, domain_view};

/// The server's public addresses, for the records of `shown` — not asked
/// for when every domain is local (they have no record).
async fn addresses(st: &AppState, shown: &[Domain]) -> Vec<IpAddr> {
    if shown.iter().all(Domain::is_local) {
        return Vec::new();
    }
    st.engine.public_addresses().await
}

async fn view(st: &AppState, domain: Domain) -> DomainView {
    let addresses = addresses(st, std::slice::from_ref(&domain)).await;
    domain_view(&st.config, domain, &addresses)
}

/// The hostnames of every service follow the domains: reload them for this
/// process, then let the engine move the routes. A failure there is only
/// logged: the domains are stored and the reconciler converges the routes.
async fn apply(st: &AppState) -> ApiResult<()> {
    domains::reload(&st.store, &st.config).await?;
    if let Err(e) = st.engine.refresh_domains().await {
        tracing::warn!("refreshing the routes after a domain change failed (the reconciler will retry): {e}");
    }
    Ok(())
}

/// `GET /api/v1/domains`
#[utoipa::path(
    get,
    path = "/api/v1/domains",
    tag = "domains",
    operation_id = "listServerDomains",
    summary = "List the server's domains",
    description = "The domains services are served under, the default one first: the server's base domain (`source: config`) and the connected ones. Each comes with the DNS records that point it at this server and what its last verification found.",
    responses((status = 200, description = "The domains.", body = [DomainView])),
)]
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<DomainView>>> {
    let all = st.store.list_domains().await?;
    let addresses = addresses(&st, &all).await;
    Ok(Json(all.into_iter().map(|d| domain_view(&st.config, d, &addresses)).collect()))
}

/// `POST /api/v1/domains`
#[utoipa::path(
    post,
    path = "/api/v1/domains",
    tag = "domains",
    operation_id = "connectDomain",
    summary = "Connect a domain",
    description = "Adds a domain (or a subdomain) of yours. It starts `pending`: create the `records` it comes with where its DNS is managed — one wildcard record covers every service. The server then verifies it every few seconds; once its names reach the server it is `active`, every web service and static site is served at `<service>.<name>` (with a certificate when the server has HTTPS), and it becomes the default domain if the default one was a local name. A local name (`*.localhost`, `.test`…) is served at once.",
    request_body = ConnectDomain,
    responses(
        (status = 201, description = "The connected domain, with the DNS records to create.", body = DomainView),
        (status = 400, description = "Not a domain name (a URL, a wildcard, an address), or too many domains.", body = ApiErrorBody),
        (status = 409, description = "The domain is already connected, or a service would be served at a hostname that is taken: a custom domain of another service, or the dashboard's host.", body = ApiErrorBody),
    ),
)]
pub async fn connect(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<ConnectDomain>,
) -> ApiResult<(StatusCode, Json<DomainView>)> {
    locks::detached(async move {
        let name = domains::connectable_name(&req.name)?;
        // The hostnames under the domain are claimed like custom domains
        // are: under the domain lock, so no service takes one meanwhile.
        let claim = locks::domains().await;
        let existing = st.store.list_domains().await?;
        if let Some(d) = existing.iter().find(|d| d.name == name) {
            return Err(Error::conflict(match d.source {
                DomainSource::Config => format!("'{name}' is the base domain of this server"),
                DomainSource::Connected => format!("domain '{name}' is already connected"),
            })
            .into());
        }
        if existing.iter().filter(|d| d.source == DomainSource::Connected).count() >= domains::MAX_DOMAINS {
            return Err(Error::invalid(format!(
                "a server serves its services under {} connected domains at most: disconnect one first",
                domains::MAX_DOMAINS
            ))
            .into());
        }
        checks::check_domain_free(&st.config, &st.store.list_services().await?, &name)?;
        let domain = Domain::new(name, DomainSource::Connected);
        st.store.create_domain(&domain).await?;
        domains::reload(&st.store, &st.config).await?;
        drop(claim);
        tracing::info!(domain = %domain.name, status = %domain.status, "connected a domain");
        apply(&st).await?;
        // The verifier may already have looked at it.
        let domain = st.store.get_domain(&domain.id).await?.unwrap_or(domain);
        Ok((StatusCode::CREATED, Json(view(&st, domain).await)))
    })
    .await
}

/// `GET /api/v1/domains/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/domains/{id}",
    tag = "domains",
    operation_id = "getServerDomain",
    summary = "Get a domain",
    params(("id" = String, Path, description = "Domain id or name.")),
    responses(
        (status = 200, description = "The domain.", body = DomainView),
        (status = 404, description = "No such domain.", body = ApiErrorBody),
    ),
)]
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<DomainView>> {
    let domain = st.store.require_domain(&id).await?;
    Ok(Json(view(&st, domain).await))
}

/// `PATCH /api/v1/domains/{id}`
#[utoipa::path(
    patch,
    path = "/api/v1/domains/{id}",
    tag = "domains",
    operation_id = "updateServerDomain",
    summary = "Make a domain the default",
    description = "`is_default: true` makes it the domain of the URL every service is shown and linked with (`url`, `FERRY_EXTERNAL_URL`); the services stay served under the other domains too. Running services read the new `FERRY_EXTERNAL_URL` at their next deploy or restart.",
    params(("id" = String, Path, description = "Domain id or name.")),
    request_body = UpdateDomain,
    responses(
        (status = 200, description = "The updated domain.", body = DomainView),
        (status = 400, description = "`is_default: false`: make another domain the default instead.", body = ApiErrorBody),
        (status = 404, description = "No such domain.", body = ApiErrorBody),
        (status = 409, description = "The domain is not served yet (its DNS doesn't point at this server).", body = ApiErrorBody),
    ),
)]
pub async fn update(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateDomain>,
) -> ApiResult<Json<DomainView>> {
    locks::detached(async move {
        let domain = st.store.require_domain(&id).await?;
        match req.is_default {
            Some(true) if !domain.is_served() => {
                return Err(Error::conflict(format!(
                    "domain '{}' is not served yet (its DNS doesn't point at this server): it can be the default one once it is",
                    domain.name
                ))
                .into());
            }
            Some(true) if !domain.is_default => {
                st.store.set_default_domain(&domain.id).await?;
                tracing::info!(domain = %domain.name, "made a domain the default one");
                apply(&st).await?;
            }
            Some(false) => {
                return Err(
                    Error::invalid("a server always has a default domain: make another domain the default instead").into()
                );
            }
            _ => {}
        }
        let domain = st.store.require_domain(&domain.id).await?;
        Ok(Json(view(&st, domain).await))
    })
    .await
}

/// `POST /api/v1/domains/{id}/verify`
#[utoipa::path(
    post,
    path = "/api/v1/domains/{id}/verify",
    tag = "domains",
    operation_id = "verifyDomain",
    summary = "Verify a domain now",
    description = "Checks at once, instead of at the server's next pass, whether the names under the domain reach this server: what a made-up name under it resolves to (`dns`), and whether this server answers a request for it on its public HTTP port (`http`). The answer is the domain with its new `status` and `checks`. A local domain has nothing to verify and is returned as it is.",
    params(("id" = String, Path, description = "Domain id or name.")),
    responses(
        (status = 200, description = "The domain, with what the verification found.", body = DomainView),
        (status = 404, description = "No such domain.", body = ApiErrorBody),
    ),
)]
pub async fn verify(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<DomainView>> {
    locks::detached(async move {
        let domain = st.store.require_domain(&id).await?;
        let domain = st.engine.verify_domain(&domain.id).await?;
        // Verified: the engine serves the services under it; this process
        // must know its hostnames too.
        domains::reload(&st.store, &st.config).await?;
        Ok(Json(view(&st, domain).await))
    })
    .await
}

/// `DELETE /api/v1/domains/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{id}",
    tag = "domains",
    operation_id = "disconnectDomain",
    summary = "Disconnect a domain",
    description = "The services stop being served under the domain at once (their other hostnames and their custom domains stay), and its certificates are no longer renewed. When it was the default domain, the base domain is again. The DNS records stay where they were created. The base domain of the server (`source: config`) can't be removed here: it is set by `--base-domain`.",
    params(("id" = String, Path, description = "Domain id or name.")),
    responses(
        (status = 204, description = "Disconnected."),
        (status = 404, description = "No such domain.", body = ApiErrorBody),
        (status = 409, description = "It is the base domain of the server.", body = ApiErrorBody),
    ),
)]
pub async fn disconnect(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<StatusCode> {
    locks::detached(async move {
        let domain = st.store.require_domain(&id).await?;
        if domain.source == DomainSource::Config {
            return Err(Error::conflict(format!(
                "'{}' is the base domain of this server: it is set by --base-domain when ferryd starts, not removed here",
                domain.name
            ))
            .into());
        }
        st.store.delete_domain(&domain.id).await?;
        tracing::info!(domain = %domain.name, "disconnected a domain");
        apply(&st).await?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// `GET /api/v1/certificates`
#[utoipa::path(
    get,
    path = "/api/v1/certificates",
    tag = "domains",
    operation_id = "listCertificates",
    summary = "List the certificates of the routed hosts",
    description = "One entry per hostname the proxy routes — the dashboard's, then every hostname of every web service and static site (under each served domain, and its custom domains) — with where its certificate stands: `issued` (and until when), `pending` or `issuing` (a new host gets its certificate within seconds of being routed), `failed` with the reason and the time of the next attempt, `local` for names no authority certifies, `disabled` when the server runs without HTTPS.",
    responses((status = 200, description = "The certificates.", body = [CertificateView])),
)]
pub async fn certificates(State(st): State<AppState>) -> ApiResult<Json<Vec<CertificateView>>> {
    let manager = st.certificates.as_deref();
    let mut out = Vec::new();
    if let Some(host) = &st.config.dashboard_host {
        out.push(certificate_view(manager, host.clone(), None));
    }
    for service in st.store.list_services().await? {
        for host in st.config.service_hosts(&service) {
            out.push(certificate_view(manager, host, Some(service.name.clone())));
        }
    }
    Ok(Json(out))
}
