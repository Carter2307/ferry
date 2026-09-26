//! Server info, liveness, dashboard and fallbacks.

use std::net::SocketAddr;

use axum::Json;
use axum::extract::{OriginalUri, State};
use axum::response::{IntoResponse, Response};
use ferry_core::dto::ServerInfo;
use http::{HeaderValue, Uri, header};

use crate::error::ApiError;
use crate::{AppState, DASHBOARD_HTML};

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
    })
}

/// `GET /healthz`
pub async fn healthz() -> &'static str {
    "ok"
}

/// `GET /` and `GET /index.html`
pub async fn dashboard() -> Response {
    let mut resp = DASHBOARD_HTML.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    resp
}

/// Unknown `/api` route (the full path: the nested router only sees the part
/// after `/api`).
pub async fn api_not_found(OriginalUri(uri): OriginalUri) -> ApiError {
    ApiError::not_found(format!("no API route for {}", uri.path()))
}

/// Unknown route outside `/api`.
pub async fn not_found(uri: Uri) -> ApiError {
    ApiError::not_found(format!("{} not found", uri.path()))
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
