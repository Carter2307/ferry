//! `/api/v1/datastores` — managed Postgres / Redis.

use axum::Json;
use axum::extract::State;
use ferry_core::dto::{CreateDatastore, DatastoreView};
use ferry_core::{Datastore, DatastoreKind, Error, validate};
use http::StatusCode;

use crate::AppState;
use crate::checks::{self, RefKind};
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiPath, ApiQuery, DeleteQuery};
use crate::locks;
use crate::ops;
use crate::views::datastore_view;

/// `GET /api/v1/datastores`
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
    Ok(ds)
}

/// `POST /api/v1/datastores`
pub async fn create(
    State(st): State<AppState>,
    ApiJson(req): ApiJson<CreateDatastore>,
) -> ApiResult<(StatusCode, Json<DatastoreView>)> {
    let ds = new_datastore(&req)?;
    // Row + provisioning: a client disconnect must not leave a row that is
    // `creating` forever.
    locks::detached(async move {
        st.store.create_datastore(&ds).await?;
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
pub async fn get(State(st): State<AppState>, ApiPath(id): ApiPath<String>) -> ApiResult<Json<DatastoreView>> {
    let ds = st.store.require_datastore(&id).await?;
    Ok(Json(datastore_view(&st.config, ds)))
}

/// `DELETE /api/v1/datastores/{id}?force=`
///
/// Refused (409) while services reference the datastore in their env
/// (`${{datastore.NAME...}}`): they would fail every later deploy / restart.
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
