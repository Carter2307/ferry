//! `/api/v1/datastores` — managed Postgres / Redis.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{ApiErrorBody, CreateDatastore, DatastoreView, UpdateDatastore};
use ferry_core::{Datastore, DatastoreKind, DatastoreStatus, Error, resources, validate};
use http::StatusCode;

use crate::AppState;
use crate::checks::{self, RefKind};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, DeleteQuery};
use crate::locks;
use crate::ops;
use crate::views::datastore_view;

/// `GET /api/v1/datastores`
#[utoipa::path(
    get,
    path = "/api/v1/datastores",
    tag = "datastores",
    operation_id = "listDatastores",
    summary = "List datastores",
    responses((status = 200, description = "All datastores with their connection info.", body = [DatastoreView])),
)]
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<DatastoreView>>> {
    let all = st.store.list_datastores().await?;
    Ok(Json(all.into_iter().map(|d| datastore_view(&st.config, d)).collect()))
}

fn trimmed(o: &Option<String>) -> Option<String> {
    o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Build (and validate) the row of a new datastore.
fn new_datastore(req: &CreateDatastore) -> Result<Datastore, Error> {
    let name = req.name.trim();
    validate::resource_name(name)?;
    let mut ds = Datastore::new(name, req.kind);
    if let Some(v) = trimmed(&req.version) {
        checks::datastore_version(&v)?;
        ds.version = v;
    }
    let database = trimmed(&req.database);
    let username = trimmed(&req.username);
    match req.kind {
        DatastoreKind::Postgres => {
            if let Some(db) = database {
                checks::pg_identifier("database", &db)?;
                ds.database = Some(db.clone());
                ds.username = db;
            }
            if let Some(u) = username {
                checks::pg_identifier("username", &u)?;
                ds.username = u;
            }
        }
        DatastoreKind::Redis => {
            if database.is_some() || username.is_some() {
                return Err(Error::invalid("database and username are only supported for postgres"));
            }
        }
    }
    // 0 = the server default, as in PATCH.
    (ds.memory_limit_mb, ds.cpu_limit) = checked_limits(req.memory_limit_mb, req.cpu_limit)?;
    Ok(ds)
}

/// Explicit limits of a request (`None` = the server default, and so is 0),
/// with the CPU limit rounded to 0.01 like services' (see
/// `validate::normalize_service`), checked.
fn checked_limits(memory_mb: Option<u32>, cpus: Option<f64>) -> Result<(Option<u32>, Option<f64>), Error> {
    let memory_mb = memory_mb.filter(|m| *m != 0);
    let cpus = cpus.filter(|c| *c != 0.0).map(resources::round_cpus);
    resources::validate(memory_mb, cpus)?;
    Ok((memory_mb, cpus))
}

/// `POST /api/v1/datastores`
#[utoipa::path(
    post,
    path = "/api/v1/datastores",
    tag = "datastores",
    operation_id = "createDatastore",
    summary = "Create a datastore",
    description = "Creates the row (`creating`) and provisions the container. A failed provisioning still answers 201, with status `failed` and the reason in `error`. Datastores and services share one namespace of names. `memory_limit_mb` (MiB, 16 MiB to 1 TiB) and `cpu_limit` (CPUs, 0.01 to 512, rounded to 0.01) limit the container; omitted or `0` = the server default (see `GET /api/v1/info`).",
    request_body = CreateDatastore,
    responses(
        (status = 201, description = "The new datastore.", body = DatastoreView),
        (status = 400, description = "Invalid name, version, database, username or resource limits.", body = ApiErrorBody),
        (status = 409, description = "The name is already taken.", body = ApiErrorBody),
    ),
)]
pub async fn create(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<CreateDatastore>,
) -> ApiResult<(StatusCode, Json<DatastoreView>)> {
    let ds = new_datastore(&req)?;
    // Row + provisioning: a client disconnect must not leave a row that is
    // `creating` forever.
    locks::detached(async move {
        {
            // Datastores share their namespace with services.
            let _names = locks::names().await;
            st.store.create_datastore(&ds).await?;
        }
        tracing::info!(datastore = %ds.name, kind = %ds.kind, "created datastore");
        let ds = match st.engine.provision_datastore(&ds.id).await {
            Ok(()) => st.store.require_datastore(&ds.id).await?,
            Err(e) => {
                tracing::warn!(datastore = %ds.name, "provisioning failed: {e}");
                match ops::mark_datastore_failed(&st.store, &ds.id, &e).await {
                    Some(ds) => ds,
                    None => return Err(e.into()),
                }
            }
        };
        Ok((StatusCode::CREATED, Json(datastore_view(&st.config, ds))))
    })
    .await
}

/// `GET /api/v1/datastores/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/datastores/{id}",
    tag = "datastores",
    operation_id = "getDatastore",
    summary = "Get a datastore",
    params(("id" = String, Path, description = "Datastore id or name.")),
    responses(
        (status = 200, description = "The datastore with its connection info.", body = DatastoreView),
        (status = 404, description = "No such datastore.", body = ApiErrorBody),
    ),
)]
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<DatastoreView>> {
    let ds = st.store.require_datastore(&id).await?;
    Ok(Json(datastore_view(&st.config, ds)))
}

/// `PATCH /api/v1/datastores/{id}` — resource limits.
///
/// The new limits are stored, then applied to the running container in
/// place (no restart). A datastore still `creating` only gets the row
/// updated: provisioning creates the container with the stored limits. A
/// `failed` one has nothing running to update.
#[utoipa::path(
    patch,
    path = "/api/v1/datastores/{id}",
    tag = "datastores",
    operation_id = "updateDatastore",
    summary = "Change a datastore's resource limits",
    description = "Every field is optional: `memory_limit_mb` (MiB, 16 MiB to 1 TiB) and `cpu_limit` (CPUs, 0.01 to 512, rounded to 0.01); `0` = back to the server default (see `GET /api/v1/info`). The limits are applied to the running container in place, without a restart (a memory limit below what the datastore currently uses can fail, or get it killed for running out of memory). A datastore that is still `creating` gets them when its container is created. When applying them fails after they were saved, the error message says so.",
    params(("id" = String, Path, description = "Datastore id or name.")),
    request_body = UpdateDatastore,
    responses(
        (status = 200, description = "The updated datastore.", body = DatastoreView),
        (status = 400, description = "Resource limits out of range.", body = ApiErrorBody),
        (status = 404, description = "No such datastore.", body = ApiErrorBody),
    ),
)]
pub async fn update(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateDatastore>,
) -> ApiResult<Json<DatastoreView>> {
    // Row + container update: a client disconnect must not stop in between.
    locks::detached(update_datastore(st, id, req)).await
}

async fn update_datastore(st: AppState, id: String, req: UpdateDatastore) -> ApiResult<Json<DatastoreView>> {
    let datastore_id = st.store.require_datastore(&id).await?.id;
    // Read under the lock: requests changing one field keep the other.
    let _row_guard = locks::owner(&datastore_id).await;
    let current = st.store.require_datastore(&datastore_id).await?;
    let memory_mb = req.memory_limit_mb.or(current.memory_limit_mb);
    let cpus = req.cpu_limit.or(current.cpu_limit);
    let (memory_mb, cpus) = checked_limits(memory_mb, cpus)?;
    let ds = if (memory_mb, cpus) != (current.memory_limit_mb, current.cpu_limit) {
        let ds = st.store.set_datastore_limits(&datastore_id, memory_mb, cpus).await?;
        let limits = st.config.limits(memory_mb, cpus);
        tracing::info!(
            datastore = %ds.name,
            memory = %limits.memory_mb.map_or_else(|| "unlimited".to_string(), resources::format_memory_mb),
            cpus = %limits.cpus.map_or_else(|| "unlimited".to_string(), resources::format_cpus),
            "changed datastore limits"
        );
        ds
    } else {
        current
    };
    // Also when nothing changed: re-sending the limits retries applying
    // them after a failure.
    if ds.status == DatastoreStatus::Available {
        st.engine.update_datastore_limits(&ds.id).await.map_err(|e| {
            ApiError::from(e)
                .prefixed(format!("limits of '{}' saved, but applying them to its container failed: ", ds.name))
        })?;
    }
    Ok(Json(datastore_view(&st.config, ds)))
}

/// `DELETE /api/v1/datastores/{id}?force=`
///
/// Refused (409) while services reference the datastore in their env
/// (`${{datastore.NAME...}}`): they would fail every later deploy / restart.
#[utoipa::path(
    delete,
    path = "/api/v1/datastores/{id}",
    tag = "datastores",
    operation_id = "deleteDatastore",
    summary = "Delete a datastore",
    description = "Removes its container **and its data volume**. Refused while services reference it in their env (`${{datastore.NAME...}}`), unless `force=true`.",
    params(
        ("id" = String, Path, description = "Datastore id or name."),
        ("force" = Option<bool>, Query, description = "Delete even though services reference it (they will fail to deploy or restart)."),
    ),
    responses(
        (status = 204, description = "Deleted."),
        (status = 404, description = "No such datastore.", body = ApiErrorBody),
        (status = 409, description = "Services reference it (the message lists them).", body = ApiErrorBody),
    ),
)]
pub async fn delete(
    State(st): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<DeleteQuery>,
) -> ApiResult<StatusCode> {
    let ds = st.store.require_datastore(&id).await?;
    let users = checks::referencing_services(&st.store, RefKind::Datastore, &ds.name, None).await?;
    if !users.is_empty() {
        if !q.force {
            return Err(Error::conflict(format!(
                "datastore '{}' is referenced by {}; they would fail to deploy or restart without it. Remove the references first, or delete with force=true",
                ds.name,
                users.join(", ")
            ))
            .into());
        }
        tracing::warn!(datastore = %ds.name, users = %users.join(", "), "deleting a datastore services reference");
    }
    st.engine.delete_datastore(&ds.id).await?;
    tracing::info!(datastore = %ds.name, "deleted datastore");
    Ok(StatusCode::NO_CONTENT)
}
