//! Job runs (cron runs and one-off jobs).

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use ferry_core::dto::RunJobRequest;
use ferry_core::{Error, JobRun, JobTrigger, ServiceType};
use http::StatusCode;

use crate::AppState;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath, ApiQuery, FollowQuery, LimitQuery};
use crate::sse;

/// `GET /api/v1/services/{id}/jobs?limit=20` (newest first)
pub async fn list(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<JobRun>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(st.store.list_job_runs(&svc.id, q.get()).await?))
}

/// `POST /api/v1/services/{id}/jobs`
pub async fn run(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<RunJobRequest>,
) -> ApiResult<(StatusCode, Json<JobRun>)> {
    let svc = st.store.require_service(&id).await?;
    let command = req.command.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if command.is_none() && svc.service_type != ServiceType::CronJob {
        return Err(Error::invalid(format!(
            "a command is required to run a job on '{}' (only cron jobs have a default command)",
            svc.name
        ))
        .into());
    }
    let job = st.engine.run_job(&svc.id, command, JobTrigger::Manual).await?;
    tracing::info!(service = %svc.name, job = %job.id, "started job");
    Ok((StatusCode::ACCEPTED, Json(job)))
}

/// `GET /api/v1/jobs/{job_id}`
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<JobRun>> {
    Ok(Json(st.store.require_job_run(&id).await?))
}

/// `GET /api/v1/jobs/{job_id}/logs?follow=` (SSE, always finite)
pub async fn logs(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<FollowQuery>,
) -> ApiResult<Response> {
    let job = st.store.require_job_run(&id).await?;
    let stream = st.engine.job_logs(&job.id, q.follow).await?;
    Ok(sse::log_response(stream, true))
}
