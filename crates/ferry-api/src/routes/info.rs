//! Server info, liveness and API fallbacks.

use std::net::SocketAddr;

use axum::Json;
use axum::extract::{OriginalUri, State};
use ferry_core::dto::ServerInfo;

use crate::AppState;
use crate::error::ApiError;

/// `http://<host>[:port]` for a listen address; unspecified IPs become `localhost`.
pub fn url_for_addr(addr: SocketAddr) -> String {
    let host = if addr.ip().is_unspecified() {
        "localhost".to_string()
    } else if addr.is_ipv6() {
        format!("[{}]", addr.ip())
    } else {
        addr.ip().to_string()
    };
    if addr.port() == 80 { format!("http://{host}") } else { format!("http://{host}:{}", addr.port()) }
}

/// `GET /api/v1/info`
#[utoipa::path(
    get,
    path = "/api/v1/info",
    tag = "info",
    operation_id = "getServerInfo",
    summary = "Server info",
    description = "Version, base domain, proxy and dashboard URLs, TLS and GitHub webhook status, Docker version. Clients use it to validate a token.",
    responses((status = 200, description = "Server info.", body = ServerInfo)),
)]
pub async fn info(State(st): State<AppState>) -> Json<ServerInfo> {
    let cfg = &st.config;
    Json(ServerInfo {
        version: ferry_core::VERSION.to_string(),
        base_domain: cfg.base_domain.clone(),
        proxy_url: url_for_addr(cfg.proxy_addr),
        tls_enabled: cfg.tls_enabled(),
        dashboard_url: cfg.dashboard_url(),
        github_webhook_enabled: cfg.github_webhook_secret.as_deref().is_some_and(|s| !s.is_empty()),
        docker_version: st.docker_version.clone(),
        default_memory_limit_mb: cfg.default_memory_limit_mb,
        default_cpu_limit: cfg.default_cpu_limit,
        docker_cpus: st.docker_cpus,
        docker_memory_bytes: st.docker_memory_bytes,
    })
}

/// `GET /healthz`
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "info",
    operation_id = "healthz",
    summary = "Liveness probe",
    description = "Answers `ok` while the server runs. No token needed.",
    responses((status = 200, description = "The server is up.", body = String, content_type = "text/plain", example = "ok")),
)]
pub async fn healthz() -> &'static str {
    "ok"
}

/// Unknown `/api` route (the full path: the nested router only sees the part
/// after `/api`).
pub async fn api_not_found(OriginalUri(uri): OriginalUri) -> ApiError {
    ApiError::not_found(format!("no API route for {}", uri.path()))
}

/// Known route, wrong method.
pub async fn method_not_allowed() -> ApiError {
    ApiError::method_not_allowed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_urls() {
        assert_eq!(url_for_addr("0.0.0.0:8080".parse().unwrap()), "http://localhost:8080");
        assert_eq!(url_for_addr("[::]:80".parse().unwrap()), "http://localhost");
        assert_eq!(url_for_addr("192.168.1.5:8080".parse().unwrap()), "http://192.168.1.5:8080");
        assert_eq!(url_for_addr("127.0.0.1:8080".parse().unwrap()), "http://127.0.0.1:8080");
        assert_eq!(url_for_addr("[fe80::1]:81".parse().unwrap()), "http://[fe80::1]:81");
    }
}
