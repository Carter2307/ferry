//! Deploys: history, manual deploys, uploads, cancel, logs.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Request, State};
use axum::response::Response;
use ferry_core::dto::{ApiErrorBody, TriggerDeploy};
use ferry_core::{Deploy, DeployRequest, DeploySource, DeployTrigger, Error, git, ids, validate};
use futures::StreamExt;
use http::{StatusCode, header};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, FollowQuery, LimitQuery, de_flag};
use crate::{AppState, UPLOAD_LIMIT, sse};

/// `GET /api/v1/services/{id}/deploys?limit=20` (newest first)
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}/deploys",
    tag = "deploys",
    operation_id = "listDeploys",
    summary = "List a service's deploys",
    description = "Newest first.",
    params(("id" = String, Path, description = "Service id or name."), LimitQuery),
    responses(
        (status = 200, description = "The deploys, newest first.", body = [Deploy]),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn list(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<Deploy>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(st.store.list_deploys(&svc.id, q.get()).await?))
}

/// `POST /api/v1/services/{id}/deploys`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/deploys",
    tag = "deploys",
    operation_id = "triggerDeploy",
    summary = "Deploy a service",
    description = "Queues a manual deploy of the configured source: the branch head (or `commit`) of the repository, or the image. An empty body deploys with the defaults.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = TriggerDeploy,
    responses(
        (status = 202, description = "The deploy, queued.", body = Deploy),
        (status = 400, description = "Invalid commit, a commit for a service that doesn't deploy from git, or no source to deploy.", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "The service is suspended or being deleted, or the server is shutting down.", body = ApiErrorBody),
    ),
)]
pub async fn trigger(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<TriggerDeploy>,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.require_service(&id).await?;
    let commit = req.commit.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if let Some(c) = &commit {
        validate::commit(c)?;
        if svc.repo_url.is_none() {
            return Err(Error::invalid(format!(
                "service '{}' doesn't deploy from git: a commit can't be chosen",
                svc.name
            ))
            .into());
        }
    }
    let d = st
        .engine
        .deploy(
            &svc.id,
            DeployRequest { trigger: DeployTrigger::Manual, source: None, commit, clear_cache: req.clear_cache },
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(d)))
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadQuery {
    /// Build without the build cache.
    #[serde(default, deserialize_with = "de_flag")]
    pub clear_cache: bool,
}

/// Removes the file on drop unless disarmed (so every early return and
/// client disconnect cleans up the partial upload).
struct TempFile {
    path: PathBuf,
    armed: bool,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let path = std::mem::take(&mut self.path);
        let remove = move || {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), "removing upload failed: {e}");
            }
        };
        // Don't block the async worker thread with file system calls.
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(remove);
            }
            Err(_) => remove(),
        }
    }
}

/// Stream `body` into `path`, enforcing `limit` and the gzip magic bytes.
async fn save_upload(req: Request, path: &Path, limit: usize) -> Result<u64, ApiError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| Error::internal(format!("creating upload file {}: {e}", path.display())))?;
    let mut body = axum::RequestExt::into_limited_body(req).into_data_stream();
    let mut total: u64 = 0;
    let mut head: Vec<u8> = Vec::with_capacity(2);
    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) if is_length_limit_error(&e) => {
                return Err(ApiError::payload_too_large(format!("upload exceeds {} MiB", limit / (1024 * 1024))));
            }
            Err(e) => return Err(ApiError::bad_request(format!("reading the upload failed: {e}"))),
        };
        total += chunk.len() as u64;
        if total > limit as u64 {
            return Err(ApiError::payload_too_large(format!("upload exceeds {} MiB", limit / (1024 * 1024))));
        }
        if head.len() < 2 {
            head.extend(chunk.iter().take(2 - head.len()));
            if head.len() == 2 && head != [0x1f, 0x8b] {
                return Err(not_gzip());
            }
        }
        file.write_all(&chunk).await.map_err(|e| Error::internal(format!("writing upload {}: {e}", path.display())))?;
    }
    if head.len() < 2 {
        return Err(not_gzip());
    }
    file.flush().await.map_err(|e| Error::internal(format!("writing upload {}: {e}", path.display())))?;
    Ok(total)
}

fn not_gzip() -> ApiError {
    ApiError::bad_request("the upload must be a gzip-compressed tar archive (.tar.gz)")
}

/// `POST /api/v1/services/{id}/deploys/upload?clear_cache=` — raw `.tar.gz` body.
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/deploys/upload",
    tag = "deploys",
    operation_id = "uploadDeploy",
    summary = "Deploy uploaded source code",
    description = "Deploys a `.tar.gz` of the source tree (what `ferry up` sends) to a service without a repository or image; the deploy has trigger `upload` and an `archive` source.",
    params(("id" = String, Path, description = "Service id or name."), UploadQuery),
    request_body(
        content = inline(crate::openapi::SourceArchive),
        content_type = "application/gzip",
        description = "The raw gzip-compressed tar archive (at most 512 MiB).",
    ),
    responses(
        (status = 202, description = "The deploy, queued.", body = Deploy),
        (status = 400, description = "The body isn't a gzip archive.", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "The service deploys from a repository or an image: clear its source first.", body = ApiErrorBody),
        (status = 413, description = "The archive exceeds 512 MiB.", body = ApiErrorBody),
    ),
)]
pub async fn upload(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<UploadQuery>,
    req: Request,
) -> ApiResult<(StatusCode, Json<Deploy>)> {
    let svc = st.store.require_service(&id).await?;
    // The next plain deploy would silently switch back to the configured
    // source, and the service would keep reporting it while running the upload.
    let source = match (&svc.image, &svc.repo_url) {
        (Some(image), _) => Some(format!("the image '{image}'")),
        (None, Some(repo)) => Some(format!("the git repository {}", git::redact_url(repo))),
        (None, None) => None,
    };
    if let Some(source) = source {
        return Err(Error::conflict(format!(
            "service '{0}' deploys from {source}; uploaded code can only be deployed to services without a repo or image. Clear its source first (ferry update {0} --repo \"\" --image \"\"), or deploy the configured source instead",
            svc.name
        ))
        .into());
    }
    if let Some(len) = req.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
        && len > UPLOAD_LIMIT as u64
    {
        return Err(ApiError::payload_too_large(format!("upload exceeds {} MiB", UPLOAD_LIMIT / (1024 * 1024))));
    }
    let dir = st.config.uploads_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::internal(format!("creating uploads directory {}: {e}", dir.display())))?;
    let path = dir.join(format!("{}.tar.gz", ids::new_id("upl")));
    let mut guard = TempFile { path: path.clone(), armed: true };
    let size = save_upload(req, &path, UPLOAD_LIMIT).await?;
    tracing::info!(service = %svc.name, bytes = size, path = %path.display(), "received source upload");
    let request = DeployRequest {
        trigger: DeployTrigger::Upload,
        source: Some(DeploySource::Archive { path: path.to_string_lossy().into_owned() }),
        commit: None,
        clear_cache: q.clear_cache,
    };
    let d = st.engine.deploy(&svc.id, request).await?;
    guard.armed = false;
    Ok((StatusCode::ACCEPTED, Json(d)))
}

/// `GET /api/v1/deploys/{deploy_id}`
#[utoipa::path(
    get,
    path = "/api/v1/deploys/{deploy_id}",
    tag = "deploys",
    operation_id = "getDeploy",
    summary = "Get a deploy",
    params(("deploy_id" = String, Path, description = "Deploy id.")),
    responses(
        (status = 200, description = "The deploy.", body = Deploy),
        (status = 404, description = "No such deploy.", body = ApiErrorBody),
    ),
)]
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<Deploy>> {
    Ok(Json(st.store.require_deploy(&id).await?))
}

/// `POST /api/v1/deploys/{deploy_id}/cancel`
#[utoipa::path(
    post,
    path = "/api/v1/deploys/{deploy_id}/cancel",
    tag = "deploys",
    operation_id = "cancelDeploy",
    summary = "Cancel a deploy",
    description = "Stops a queued or running deploy (status `canceled`); the live deploy keeps serving.",
    params(("deploy_id" = String, Path, description = "Deploy id.")),
    responses(
        (status = 200, description = "The canceled deploy.", body = Deploy),
        (status = 404, description = "No such deploy.", body = ApiErrorBody),
        (status = 409, description = "The deploy already finished.", body = ApiErrorBody),
    ),
)]
pub async fn cancel(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<Deploy>> {
    let d = st.store.require_deploy(&id).await?;
    Ok(Json(st.engine.cancel_deploy(&d.id).await?))
}

/// `GET /api/v1/deploys/{deploy_id}/logs?follow=` (SSE, always finite)
#[utoipa::path(
    get,
    path = "/api/v1/deploys/{deploy_id}/logs",
    tag = "deploys",
    operation_id = "streamDeployLogs",
    summary = "Build and deploy logs (SSE)",
    params(("deploy_id" = String, Path, description = "Deploy id."), FollowQuery),
    responses(
        (status = 200, description = crate::openapi::SSE_FINITE_LOGS, content_type = "text/event-stream", body = String, example = "event: log\ndata: {\"ts\":\"2026-01-01T12:00:00Z\",\"stream\":\"system\",\"line\":\"==> Build succeeded\"}\n\nevent: end\ndata: \n\n"),
        (status = 404, description = "No such deploy.", body = ApiErrorBody),
    ),
)]
pub async fn logs(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<FollowQuery>,
) -> ApiResult<Response> {
    let d = st.store.require_deploy(&id).await?;
    let stream = st.engine.deploy_logs(&d.id, q.follow).await?;
    Ok(sse::log_response(stream, true, &st.shutdown))
}

/// Detect `http_body_util::LengthLimitError` behind axum's body error.
fn is_length_limit_error(e: &axum::Error) -> bool {
    use std::error::Error as _;
    let mut source: Option<&(dyn std::error::Error + 'static)> = e.source();
    while let Some(s) = source {
        if s.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        source = s.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn request(body: impl Into<Body>) -> Request {
        Request::builder().method("POST").uri("/").body(body.into()).unwrap()
    }

    #[tokio::test]
    async fn saves_gzip_and_rejects_others() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.tar.gz");
        let n = save_upload(request(vec![0x1f, 0x8b, 8, 0, 1, 2, 3]), &p, 1024).await.unwrap();
        assert_eq!(n, 7);
        assert_eq!(std::fs::read(&p).unwrap().len(), 7);

        let p2 = dir.path().join("b.tar.gz");
        let err = save_upload(request("hello world"), &p2, 1024).await.unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        let err = save_upload(request(Body::empty()), &p2, 1024).await.unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        let err = save_upload(request(vec![0x1f; 1]), &p2, 1024).await.unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);

        let mut big = vec![0x1f, 0x8b];
        big.extend(std::iter::repeat_n(0u8, 100));
        let err = save_upload(request(big), &p2, 10).await.unwrap_err();
        assert_eq!(err.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn temp_file_guard_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.tar.gz");
        std::fs::write(&p, b"x").unwrap();
        drop(TempFile { path: p.clone(), armed: true });
        for _ in 0..100 {
            if !p.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!p.exists());
        let keep = dir.path().join("keep.tar.gz");
        std::fs::write(&keep, b"x").unwrap();
        drop(TempFile { path: keep.clone(), armed: false });
        assert!(keep.exists());
    }

    #[tokio::test]
    async fn detects_length_limit_errors() {
        use http_body_util::BodyExt;
        let limited = http_body_util::Limited::new(Body::from(vec![0u8; 64]), 8);
        let body = Body::new(limited);
        let err = body.collect().await.unwrap_err();
        assert!(is_length_limit_error(&err));
    }
}
