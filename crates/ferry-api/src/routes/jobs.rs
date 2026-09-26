//! Job runs (cron runs and one-off jobs).

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use ferry_core::dto::{ApiErrorBody, RunJobRequest};
use ferry_core::{Error, JobRun, JobTrigger, ServiceType};
use http::StatusCode;

use crate::AppState;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath, ApiQuery, FollowQuery, LimitQuery};
use crate::sse;

/// `GET /api/v1/services/{id}/jobs?limit=20` (newest first)
#[utoipa::path(
    get,
    path = "/api/v1/services/{id}/jobs",
    tag = "jobs",
    operation_id = "listJobs",
    summary = "List a service's job runs",
    description = "Cron runs and one-off jobs, newest first.",
    params(("id" = String, Path, description = "Service id or name."), LimitQuery),
    responses(
        (status = 200, description = "The job runs, newest first.", body = [JobRun]),
        (status = 404, description = "No such service.", body = ApiErrorBody),
    ),
)]
pub async fn list(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<JobRun>>> {
    let svc = st.store.require_service(&id).await?;
    Ok(Json(st.store.list_job_runs(&svc.id, q.get()).await?))
}

/// `POST /api/v1/services/{id}/jobs`
#[utoipa::path(
    post,
    path = "/api/v1/services/{id}/jobs",
    tag = "jobs",
    operation_id = "runJob",
    summary = "Run a job",
    description = "Runs a one-off command (`sh -c`) in a new container from the service's live image. Cron jobs run their own command when `command` is omitted.",
    params(("id" = String, Path, description = "Service id or name.")),
    request_body = RunJobRequest,
    responses(
        (status = 202, description = "The job run, started.", body = JobRun),
        (status = 400, description = "No command for a service that isn't a cron job.", body = ApiErrorBody),
        (status = 404, description = "No such service.", body = ApiErrorBody),
        (status = 409, description = "The service has no live deploy to run from, is suspended or being deleted.", body = ApiErrorBody),
    ),
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/jobs/{job_id}",
    tag = "jobs",
    operation_id = "getJob",
    summary = "Get a job run",
    params(("job_id" = String, Path, description = "Job run id.")),
    responses(
        (status = 200, description = "The job run.", body = JobRun),
        (status = 404, description = "No such job run.", body = ApiErrorBody),
    ),
)]
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<JobRun>> {
    Ok(Json(st.store.require_job_run(&id).await?))
}

/// `POST /api/v1/jobs/{job_id}/cancel` — stop a pending or running job
/// (`canceled`); 409 when it already finished.
#[utoipa::path(
    post,
    path = "/api/v1/jobs/{job_id}/cancel",
    tag = "jobs",
    operation_id = "cancelJob",
    summary = "Cancel a job run",
    description = "Stops a pending or running job (status `canceled`).",
    params(("job_id" = String, Path, description = "Job run id.")),
    responses(
        (status = 200, description = "The canceled job run.", body = JobRun),
        (status = 404, description = "No such job run.", body = ApiErrorBody),
        (status = 409, description = "The job already finished.", body = ApiErrorBody),
    ),
)]
pub async fn cancel(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<JobRun>> {
    let job = st.store.require_job_run(&id).await?;
    if job.status.is_terminal() {
        return Err(Error::conflict(format!("job {} already finished ({})", job.id, job.status)).into());
    }
    let job = st.engine.cancel_job(&job.id).await?;
    tracing::info!(job = %job.id, status = %job.status, "canceled job");
    Ok(Json(job))
}

/// `GET /api/v1/jobs/{job_id}/logs?follow=` (SSE, always finite)
#[utoipa::path(
    get,
    path = "/api/v1/jobs/{job_id}/logs",
    tag = "jobs",
    operation_id = "streamJobLogs",
    summary = "Job logs (SSE)",
    params(("job_id" = String, Path, description = "Job run id."), FollowQuery),
    responses(
        (status = 200, description = crate::openapi::SSE_FINITE_LOGS, content_type = "text/event-stream", body = String, example = "event: log\ndata: {\"ts\":\"2026-01-01T12:00:00Z\",\"stream\":\"system\",\"line\":\"==> Build succeeded\"}\n\nevent: end\ndata: \n\n"),
        (status = 404, description = "No such job run.", body = ApiErrorBody),
    ),
)]
pub async fn logs(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<FollowQuery>,
) -> ApiResult<Response> {
    let job = st.store.require_job_run(&id).await?;
    let stream = st.engine.job_logs(&job.id, q.follow).await?;
    Ok(sse::log_response(stream, true, &st.shutdown))
}
