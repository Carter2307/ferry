//! The OpenAPI 3.1 document of the HTTP API and its Swagger UI:
//!
//! * `GET /api/openapi.json` — the document ([`ApiDoc`]);
//! * `/api/docs` — Swagger UI (assets compiled into the binary, works
//!   offline). Its **Authorize** button takes the API token (bearer scheme).
//!
//! Both are public: the document describes the API, it grants nothing. Every
//! handler carries its own `#[utoipa::path]`; [`Security`] then marks every
//! `/api/v1` operation as requiring the bearer token (exactly what the auth
//! middleware enforces on the nested API router) and adds the responses all
//! of them share.

use axum::Router;
use axum::response::Response;
use http::{HeaderValue, header};
use utoipa::openapi::path::{Operation, ParameterIn};
use utoipa::openapi::response::ResponseBuilder;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{ContentBuilder, Ref, RefOr};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::routes::{blueprints, datastores, deploys, domains, env, env_groups, hooks, info, jobs, services};

/// Where the document is served.
pub const OPENAPI_PATH: &str = "/api/openapi.json";

/// Where Swagger UI is served (`/api/docs` redirects to `/api/docs/`).
pub const DOCS_PATH: &str = "/api/docs";

/// Name of the bearer security scheme.
pub const SECURITY_SCHEME: &str = "bearer";

const DESCRIPTION: &str = "\
The HTTP API of [Ferry](https://github.com/Carter2307/ferry), a self-hosted Render alternative. \
The `ferry` CLI and the web dashboard use exactly this API.

**Authentication.** Every `/api/v1` route needs the server's API token (printed by `ferryd` on \
first start, stored in `<data-dir>/api_token`) as `Authorization: Bearer <token>`; use the \
**Authorize** button to set it here. `GET` requests may pass it as `?access_token=<token>` \
instead, because browsers' `EventSource` can't set headers (prefer the header: query strings end \
up in logs). Webhooks (`/hooks/...`) authenticate with their own secrets, and `/healthz`, this \
document and Swagger UI need no token.

**Conventions.** JSON bodies with snake_case fields; request bodies reject unknown fields and an \
empty body counts as `{}`. `{id}` path segments accept a resource id **or** its name. Errors are \
`ApiErrorBody` documents (`{\"error\": {\"code\", \"message\"}}`) with the matching HTTP status; \
unknown `/api` routes are JSON 404s. Boolean query flags accept `true/false`, `1/0`, `yes/no`, \
`on/off`, and an empty value means `true` (`?follow`).

**Streams.** Logs and the change feed are Server-Sent Events (`text/event-stream`): see the \
`logs` operations and `GET /api/v1/events`. Idle streams get a `: keep-alive` comment every 15 s, \
and every stream ends when the server shuts down.";

/// Response description of the deploy and job log streams.
pub(crate) const SSE_FINITE_LOGS: &str = "Server-Sent Events. Every line is an `event: log` whose `data` is a \
`LogLine` JSON object (see the `LogLine` schema). The stream always ends with `event: end` (empty `data`): once \
the stored log is replayed, or with `follow=true` once the deploy or job finishes. `: keep-alive` comments \
every 15 s while idle.";

/// Response description of the runtime log stream.
pub(crate) const SSE_RUNTIME_LOGS: &str = "Server-Sent Events. Every line is an `event: log` whose `data` is a \
`LogLine` JSON object (see the `LogLine` schema). Without `follow` the stream ends with `event: end` (empty \
`data`) after the recent lines; with `follow=true` it stays open and never sends `end`. `: keep-alive` \
comments every 15 s while idle.";

/// Response description of the change feed.
pub(crate) const SSE_EVENTS: &str = "Server-Sent Events. First `event: ready` (`data: {}`) once the feed \
watches the store: changes after it are reported, so (re)fetch your data after `ready`. Then one \
`event: change` per change, whose `data` is a `ChangeEvent` JSON object `{kind, id, service_id, action}`: \
`kind` is `service`, `deploy`, `datastore`, `env_group` or `job`; `action` is `created`, `updated` or \
`deleted`; `service_id` is set for services (their own id), deploys and jobs, `null` otherwise. A subscriber \
that falls behind gets `{\"kind\": \"all\", \"id\": \"*\", \"service_id\": null, \"action\": \"resync\"}` and \
should refetch everything. `: keep-alive` comments every 15 s; the stream only ends when the server shuts \
down.";

/// The document. Build it with `ApiDoc::openapi()`.
#[derive(OpenApi)]
#[openapi(
    info(title = "Ferry API", version = env!("CARGO_PKG_VERSION"), description = DESCRIPTION),
    paths(
        info::healthz,
        info::info,
        openapi_json,
        crate::events::stream,
        services::list,
        services::create,
        services::get,
        services::update,
        services::delete,
        services::status,
        services::logs,
        services::restart,
        services::suspend,
        services::resume,
        services::scale,
        services::rollback,
        services::rotate_deploy_hook,
        deploys::list,
        deploys::trigger,
        deploys::upload,
        deploys::get,
        deploys::cancel,
        deploys::logs,
        env::list,
        env::replace,
        env::patch,
        env::link_group,
        env::unlink_group,
        domains::list,
        domains::add,
        domains::remove,
        jobs::list,
        jobs::run,
        jobs::get,
        jobs::cancel,
        jobs::logs,
        datastores::list,
        datastores::create,
        datastores::get,
        datastores::update,
        datastores::delete,
        env_groups::list,
        env_groups::create,
        env_groups::get,
        env_groups::delete,
        env_groups::replace_env,
        env_groups::patch_env,
        blueprints::apply,
        hooks::deploy_hook_get,
        hooks::deploy_hook,
        hooks::github,
    ),
    components(schemas(
        ferry_core::dto::ServerInfo,
        ferry_core::dto::ServiceView,
        ferry_core::dto::CreateService,
        ferry_core::dto::UpdateService,
        ferry_core::dto::TriggerDeploy,
        ferry_core::dto::RollbackRequest,
        ferry_core::dto::ScaleRequest,
        ferry_core::dto::ReplaceEnv,
        ferry_core::dto::PatchEnv,
        ferry_core::dto::DomainRequest,
        ferry_core::dto::RunJobRequest,
        ferry_core::dto::LinkEnvGroup,
        ferry_core::dto::InstanceStatus,
        ferry_core::dto::RuntimeStatus,
        ferry_core::dto::CreateDatastore,
        ferry_core::dto::UpdateDatastore,
        ferry_core::dto::DatastoreView,
        ferry_core::dto::CreateEnvGroup,
        ferry_core::dto::EnvGroupView,
        ferry_core::dto::ApplyBlueprint,
        ferry_core::dto::BlueprintAction,
        ferry_core::dto::BlueprintResult,
        ferry_core::dto::ApiErrorBody,
        ferry_core::dto::ApiErrorDetail,
        ferry_core::Service,
        ferry_core::ServiceType,
        ferry_core::ServiceState,
        ferry_core::Runtime,
        ferry_core::SourceKind,
        ferry_core::Deploy,
        ferry_core::DeploySource,
        ferry_core::DeployStatus,
        ferry_core::DeployTrigger,
        ferry_core::JobRun,
        ferry_core::JobStatus,
        ferry_core::JobTrigger,
        ferry_core::Datastore,
        ferry_core::DatastoreKind,
        ferry_core::DatastoreStatus,
        ferry_core::EnvVar,
        ferry_core::EnvGroup,
        ferry_core::LogLine,
        ferry_core::LogStreamKind,
        crate::events::Change,
        ChangeKind,
        ChangeAction,
        GithubHookResponse,
        SourceArchive,
    )),
    modifiers(&Security),
    tags(
        (name = "info", description = "Server info, liveness and this document."),
        (name = "services", description = "Web services, private services, background workers, static sites and cron jobs: CRUD, lifecycle actions (restart, suspend, resume, scale, rollback), status and runtime logs."),
        (name = "deploys", description = "Deploy history, manual deploys, source uploads (`ferry up`), cancellation and build logs."),
        (name = "env", description = "A service's own environment variables. `?restart=true` restarts the live service when its effective environment changed."),
        (name = "env-groups", description = "Shared variable sets linked to services (a service's own variables win over its groups'), and linking / unlinking them."),
        (name = "domains", description = "Custom domains of web services and static sites (unique across services)."),
        (name = "jobs", description = "Job runs: cron runs and one-off commands, with their logs."),
        (name = "datastores", description = "Managed Postgres and Redis instances with their connection strings and resource limits."),
        (name = "blueprints", description = "Infrastructure as code: apply a `ferry.yaml` / `render.yaml` (idempotent, with a dry run)."),
        (name = "events", description = "The change feed (Server-Sent Events) the web client uses to stay current without polling."),
        (name = "hooks", description = "Webhooks: secret deploy hook URLs and GitHub push events. They authenticate with their own secrets, never the API token."),
    )
)]
pub struct ApiDoc;

/// Marks every `/api/v1` operation as requiring the bearer token and adds
/// the responses they share: 401, 5XX (webhooks too) and, for operations
/// with a body or query parameters, 400.
pub struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            SECURITY_SCHEME,
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("API token")
                    .description(Some(
                        "The server's API token (`ferryd` prints it on first start; it is stored in \
                         `<data-dir>/api_token`). `GET` requests may send it as `?access_token=` instead.",
                    ))
                    .build(),
            ),
        );
        for (path, item) in openapi.paths.paths.iter_mut() {
            let authenticated = path.starts_with("/api/v1/");
            let fallible = authenticated || path.starts_with("/hooks/");
            let operations = [
                &mut item.get,
                &mut item.put,
                &mut item.post,
                &mut item.delete,
                &mut item.options,
                &mut item.head,
                &mut item.patch,
                &mut item.trace,
            ];
            for op in operations.into_iter().flatten() {
                if authenticated {
                    op.security = Some(vec![SecurityRequirement::new(SECURITY_SCHEME, Vec::<String>::new())]);
                    add_response(op, "401", "Missing or invalid API token.");
                }
                let has_query = op.parameters.iter().flatten().any(|p| match p {
                    RefOr::T(p) => p.parameter_in == ParameterIn::Query,
                    RefOr::Ref(_) => false,
                });
                if op.request_body.is_some() || has_query {
                    add_response(op, "400", "Malformed request: invalid JSON body or query string.");
                }
                if fallible {
                    add_response(
                        op,
                        "5XX",
                        "Server error (`docker_error` as 502, `database_error`, `internal_error`...).",
                    );
                }
            }
        }
    }
}

/// Add an `ApiErrorBody` response unless the operation documents that status.
fn add_response(op: &mut Operation, status: &str, description: &str) {
    if op.responses.responses.contains_key(status) {
        return;
    }
    let content = ContentBuilder::new().schema(Some(Ref::from_schema_name("ApiErrorBody"))).build();
    let response = ResponseBuilder::new().description(description).content("application/json", content).build();
    op.responses.responses.insert(status.to_string(), RefOr::T(response));
}

/// What a change event is about (`all` only with `resync`).
#[derive(ToSchema)]
#[schema(rename_all = "snake_case")]
pub enum ChangeKind {
    Service,
    Deploy,
    Datastore,
    EnvGroup,
    Job,
    /// Everything: sent with `resync` when a subscriber fell behind.
    All,
}

/// What happened (`resync`: refetch everything).
#[derive(ToSchema)]
#[schema(rename_all = "snake_case")]
pub enum ChangeAction {
    Created,
    Updated,
    Deleted,
    Resync,
}

/// Response of `POST /hooks/github`: `{"deploys": [...]}` for a push,
/// `{"deploys": [], "ignored": true, "reason": "..."}` for an already
/// processed delivery, `{"ok": true}` for `ping` and `{"ignored": true}` for
/// other events.
#[derive(ToSchema)]
pub struct GithubHookResponse {
    /// Deploys queued by a push (credentials redacted).
    pub deploys: Option<Vec<ferry_core::Deploy>>,
    /// The delivery wasn't acted on (not a push, or a replayed push).
    pub ignored: Option<bool>,
    /// Why a push was ignored.
    pub reason: Option<String>,
    /// Answer to `ping`.
    pub ok: Option<bool>,
}

/// A gzip-compressed tar archive of the source tree (`.tar.gz`, at most 512 MiB).
#[derive(ToSchema)]
#[schema(value_type = String, format = Binary, content_media_type = "application/gzip")]
pub struct SourceArchive(pub Vec<u8>);

/// `GET /api/openapi.json`: served by Swagger UI's router, documented here.
#[utoipa::path(
    get,
    path = "/api/openapi.json",
    tag = "info",
    operation_id = "getOpenApi",
    summary = "This OpenAPI document",
    description = "The OpenAPI 3.1 document of this API, as JSON. No token needed; Swagger UI at `/api/docs` renders it.",
    responses((status = 200, description = "This OpenAPI 3.1 document (no token needed).", body = Object)),
)]
#[allow(dead_code)]
fn openapi_json() {}

/// The unauthenticated routes serving the document and Swagger UI.
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let config = Config::new([OPENAPI_PATH])
        // No request to validator.swagger.io: the page works offline and the
        // document's URL stays private.
        .validator_url("none")
        .filter(true)
        .display_request_duration(true)
        .persist_authorization(false);
    let swagger = SwaggerUi::new(DOCS_PATH).url(OPENAPI_PATH, ApiDoc::openapi()).config(config);
    Router::from(swagger).layer(axum::middleware::map_response(harden))
}

/// Same protections as the web client's HTML: no framing, no referrer, no sniffing.
async fn harden(mut resp: Response) -> Response {
    let headers = resp.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    resp
}
