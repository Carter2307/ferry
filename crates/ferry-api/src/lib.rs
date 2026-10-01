//! # ferry-api
//!
//! The control plane's HTTP surface (axum):
//! * `/api/v1/...` — REST API (an API token or the dashboard's session, see
//!   [`auth`] and DESIGN.md §20);
//! * `/hooks/github` and `/hooks/deploy/{service_id}` — webhooks;
//! * `/api/v1/events` — SSE change feed for the web client;
//! * `/api/openapi.json` and `/api/docs` — the OpenAPI document and Swagger
//!   UI (no auth, see [`openapi`]);
//! * `/healthz` — liveness (no auth);
//! * `/` — the web client (a single-page app built from `web/` and embedded
//!   at compile time, see [`web`]), with a SPA fallback for its routes.
//!
//! It performs validation and plain data changes through the [`Store`] and
//! delegates everything with side effects to the [`Engine`] trait object.

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use ferry_core::{CancellationToken, Config, Engine, Store};
use tower_http::trace::TraceLayer;

pub mod blueprint;

mod auth;
mod checks;
mod error;
pub mod events;
mod extract;
mod locks;
pub mod openapi;
mod ops;
mod routes;
mod runtime;
pub mod setup;
mod sse;
mod views;
pub mod web;

pub use error::{ApiError, ApiResult};

/// Maximum size of an uploaded source archive (`ferry up`).
pub const UPLOAD_LIMIT: usize = 512 * 1024 * 1024;

/// Maximum size of a GitHub webhook payload (GitHub caps them at 25 MB).
pub const WEBHOOK_LIMIT: usize = 25 * 1024 * 1024;

/// Shared state of every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub engine: Arc<dyn Engine>,
    /// Docker server version (for `/api/v1/info`).
    pub docker_version: Option<String>,
    /// CPUs of the Docker host (for `/api/v1/info`).
    pub docker_cpus: Option<u32>,
    /// Total memory of the Docker host in bytes (for `/api/v1/info`).
    pub docker_memory_bytes: Option<u64>,
    /// Cancelled when the server shuts down: open SSE log streams end so the
    /// HTTP server's graceful shutdown can complete.
    pub shutdown: CancellationToken,
}

/// Build the complete router (API + hooks + web client + healthz).
pub fn router(state: AppState) -> axum::Router {
    use routes::*;

    let hub = events::Hub::new(state.store.clone(), state.shutdown.clone());
    let auth_runtime = crate::auth::Runtime::new();
    // The account, its sessions and its API tokens: the dashboard's session
    // only, whatever the request says otherwise.
    let account = Router::new()
        .route("/v1/auth/password", post(auth::change_password))
        .route("/v1/auth/sessions", get(auth::list_sessions))
        .route("/v1/auth/sessions/{id}", axum::routing::delete(auth::delete_session))
        .route("/v1/auth/tokens", get(auth::list_tokens).post(auth::create_token))
        .route("/v1/auth/tokens/{id}", axum::routing::delete(auth::delete_token))
        .route("/v1/auth/cli/{id}", get(auth::cli_get))
        .route("/v1/auth/cli/{id}/approve", post(auth::cli_approve))
        .route("/v1/auth/cli/{id}/deny", post(auth::cli_deny))
        .layer(axum::middleware::from_fn(crate::auth::require_session));
    let api = Router::new()
        .route("/v1/info", get(info::info))
        .route("/v1/events", get(events::stream).layer(axum::Extension(hub.clone())))
        // services
        .route("/v1/services", get(services::list).post(services::create))
        .route("/v1/services/{id}", get(services::get).patch(services::update).delete(services::delete))
        .route("/v1/services/{id}/status", get(services::status))
        .route("/v1/services/{id}/logs", get(services::logs))
        .route("/v1/services/{id}/restart", post(services::restart))
        .route("/v1/services/{id}/suspend", post(services::suspend))
        .route("/v1/services/{id}/resume", post(services::resume))
        .route("/v1/services/{id}/scale", post(services::scale))
        .route("/v1/services/{id}/rollback", post(services::rollback))
        .route("/v1/services/{id}/deploy-hook/rotate", post(services::rotate_deploy_hook))
        // deploys
        .route("/v1/services/{id}/deploys", get(deploys::list).post(deploys::trigger))
        .route("/v1/services/{id}/deploys/upload", post(deploys::upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)))
        .route("/v1/deploys/{deploy_id}", get(deploys::get))
        .route("/v1/deploys/{deploy_id}/cancel", post(deploys::cancel))
        .route("/v1/deploys/{deploy_id}/logs", get(deploys::logs))
        // env vars & env group links
        .route("/v1/services/{id}/env", get(env::list).put(env::replace).patch(env::patch))
        .route("/v1/services/{id}/env-groups", post(env::link_group))
        .route("/v1/services/{id}/env-groups/{group}", axum::routing::delete(env::unlink_group))
        // custom domains
        .route("/v1/services/{id}/domains", get(domains::list).post(domains::add))
        .route("/v1/services/{id}/domains/{domain}", axum::routing::delete(domains::remove))
        // jobs
        .route("/v1/services/{id}/jobs", get(jobs::list).post(jobs::run))
        .route("/v1/jobs/{job_id}", get(jobs::get))
        .route("/v1/jobs/{job_id}/cancel", post(jobs::cancel))
        .route("/v1/jobs/{job_id}/logs", get(jobs::logs))
        // datastores
        .route("/v1/datastores", get(datastores::list).post(datastores::create))
        .route("/v1/datastores/{id}", get(datastores::get).patch(datastores::update).delete(datastores::delete))
        // env groups
        .route("/v1/env-groups", get(env_groups::list).post(env_groups::create))
        .route("/v1/env-groups/{id}", get(env_groups::get).delete(env_groups::delete))
        .route("/v1/env-groups/{id}/env", axum::routing::put(env_groups::replace_env).patch(env_groups::patch_env))
        // git connections
        .route("/v1/git/connections", get(git::list).post(git::connect))
        .route("/v1/git/connections/{id}", get(git::get).delete(git::delete))
        .route("/v1/git/connections/{id}/repositories", get(git::repositories))
        .route("/v1/git/authorize", post(git::authorize))
        .route("/v1/git/callback", post(git::callback))
        .route("/v1/git/branches", get(git::branches))
        // blueprints
        .route("/v1/blueprints/apply", post(blueprints::apply))
        .merge(account)
        .method_not_allowed_fallback(info::method_not_allowed)
        .fallback(info::api_not_found)
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::require_auth));

    // What a browser or a terminal calls before it is authenticated.
    let public_api = Router::new()
        .route("/v1/auth/status", get(auth::status))
        .route("/v1/auth/setup", post(auth::setup))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/auth/cli", post(auth::cli_start))
        .route("/v1/auth/cli/{id}/token", post(auth::cli_token))
        .method_not_allowed_fallback(info::method_not_allowed);
    let api = api.merge(public_api).layer(axum::Extension(auth_runtime));

    let hooks = Router::new()
        .route("/hooks/deploy/{service_id}", get(hooks::deploy_hook_get).post(hooks::deploy_hook))
        .route("/hooks/github", post(hooks::github).layer(DefaultBodyLimit::max(WEBHOOK_LIMIT)))
        .method_not_allowed_fallback(info::method_not_allowed);

    // Everything else is the web client (files + SPA fallback); unknown
    // `/api`, `/hooks` and `/healthz` paths stay JSON 404s. The document and
    // Swagger UI are routes of their own (outside the token-checking API
    // router), so neither fallback ever sees them.
    let ui = Arc::new(web::Ui::from_env());
    let ui_fallback = move |method: http::Method, uri: http::Uri, headers: http::HeaderMap| async move {
        ui.serve(&method, &uri, &headers).await
    };

    Router::new()
        .route("/healthz", get(info::healthz))
        .merge(hooks)
        .nest("/api", api)
        .merge(openapi::router())
        .method_not_allowed_fallback(info::method_not_allowed)
        .fallback(ui_fallback)
        // API and webhook writes are reported by the change feed at once.
        .layer(axum::middleware::from_fn_with_state(hub, events::nudge_after_writes))
        .layer(TraceLayer::new_for_http().make_span_with(|req: &http::Request<axum::body::Body>| {
            // Path only: query strings may carry secrets (?access_token=, ?key=).
            tracing::info_span!("http", method = %req.method(), path = %req.uri().path())
        }))
        .with_state(state)
}
