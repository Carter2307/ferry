//! # ferry-api
//!
//! The control plane's HTTP surface (axum):
//! * `/api/v1/...` — REST API (bearer token), see DESIGN.md §API;
//! * `/hooks/github` and `/hooks/deploy/{service_id}` — webhooks;
//! * `/healthz` — liveness (no auth);
//! * `/` — the single-page web dashboard (`assets/index.html`).
//!
//! It performs validation and plain data changes through the [`Store`] and
//! delegates everything with side effects to the [`Engine`] trait object.

use std::sync::Arc;

use ferry_core::{Config, Engine, Store};

pub mod blueprint;

/// The web dashboard (single self-contained HTML file).
pub const DASHBOARD_HTML: &str = include_str!("../assets/index.html");

/// Shared state of every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub engine: Arc<dyn Engine>,
    /// Docker server version (for `/api/v1/info`).
    pub docker_version: Option<String>,
}

/// Build the complete router (API + hooks + dashboard + healthz).
pub fn router(state: AppState) -> axum::Router {
    let _ = state;
    todo!("ferry-api: router")
}
