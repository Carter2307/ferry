//! The OpenAPI 3.1 document of the HTTP API and its Swagger UI:
//!
//! * `GET /api/openapi.json` — the document ([`ApiDoc`]);
//! * `/api/docs` — Swagger UI (assets compiled into the binary, works
//!   offline). Its **Authorize** button takes an API token (bearer scheme).
//!
//! Both are public: the document describes the API, it grants nothing. Every
//! handler carries its own `#[utoipa::path]`; [`Security`] then marks every
//! `/api/v1` operation with what authenticates it (exactly what the auth
//! middleware enforces on the nested API router: an API token or the
//! dashboard's session; the session only for the account's own operations;
//! nothing for the [`PUBLIC_PATHS`]) and adds the responses all of them
//! share.

use axum::Router;
use axum::response::Response;
use http::{HeaderValue, header};
use utoipa::openapi::path::{Operation, ParameterIn};
use utoipa::openapi::response::ResponseBuilder;
use utoipa::openapi::security::{
    ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
};
use utoipa::openapi::{ContentBuilder, Ref, RefOr};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::routes::{
    auth, blueprints, custom_domains, datastores, deploys, domains, env, env_groups, git, hooks, info, jobs, services,
};

/// Where the document is served.
pub const OPENAPI_PATH: &str = "/api/openapi.json";

/// Where Swagger UI is served (`/api/docs` redirects to `/api/docs/`).
pub const DOCS_PATH: &str = "/api/docs";

/// Name of the bearer security scheme (an API token).
pub const SECURITY_SCHEME: &str = "bearer";

/// Name of the cookie security scheme (the dashboard's session).
pub const SESSION_SCHEME: &str = "session";

/// The `/api/v1` paths that need no authentication: what a browser or a
/// terminal calls before it has any.
pub const PUBLIC_PATHS: &[&str] = &[
    "/api/v1/auth/status",
    "/api/v1/auth/setup",
    "/api/v1/auth/login",
    "/api/v1/auth/logout",
    "/api/v1/auth/cli",
    "/api/v1/auth/cli/{id}/token",
];

/// The paths only the dashboard's session may call: the account itself.
pub const SESSION_PATHS_PREFIX: &str = "/api/v1/auth/";

const DESCRIPTION: &str = "\
The HTTP API of [Ferry](https://github.com/Carter2307/ferry), a self-hosted Render alternative. \
The `ferry` CLI and the web dashboard use exactly this API.

**Authentication.** Every `/api/v1` route needs an API token as `Authorization: Bearer <token>`: \
one created in the dashboard (Server → Account) or by `ferry login`, or the server token stored \
in `<data-dir>/api_token`. Use the **Authorize** button to set it here. `GET` requests may pass \
it as `?access_token=<token>` instead, because browsers' `EventSource` can't set headers (prefer \
the header: query strings end up in logs). The dashboard itself signs in to the server's account \
(`POST /api/v1/auth/login`) and is then authenticated by its session cookie; the \
operations on the account, its sessions and its tokens (`/api/v1/auth/...`) only accept that \
session. Webhooks (`/hooks/...`) authenticate with their own secrets, and `/healthz`, this \
document, Swagger UI and the calls made before signing in (the `auth` tag says which) need no \
token.

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
`kind` is `service`, `deploy`, `datastore`, `env_group`, `job`, `git_connection` or `domain`; `action` is \
`created`, `updated` or `deleted`; `service_id` is set for services (their own id), deploys and jobs, `null` otherwise. A subscriber \
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
        auth::status,
        auth::setup,
        auth::login,
        auth::logout,
        auth::change_password,
        auth::list_sessions,
        auth::delete_session,
        auth::list_tokens,
        auth::create_token,
        auth::delete_token,
        auth::cli_start,
        auth::cli_get,
        auth::cli_approve,
        auth::cli_deny,
        auth::cli_token,
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
        custom_domains::list,
        custom_domains::add,
        custom_domains::remove,
        domains::list,
        domains::connect,
        domains::get,
        domains::update,
        domains::verify,
        domains::disconnect,
        domains::certificates,
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
        git::list,
        git::authorize,
        git::callback,
        git::connect,
        git::get,
        git::delete,
        git::repositories,
        git::branches,
        blueprints::apply,
        hooks::deploy_hook_get,
        hooks::deploy_hook,
        hooks::github,
    ),
    components(schemas(
        ferry_core::dto::ServerInfo,
        ferry_core::dto::AuthStatus,
        ferry_core::dto::AuthKind,
        ferry_core::dto::UserView,
        ferry_core::dto::SetupAccount,
        ferry_core::dto::Login,
        ferry_core::dto::ChangePassword,
        ferry_core::dto::SessionView,
        ferry_core::dto::ApiTokenView,
        ferry_core::dto::CreateApiToken,
        ferry_core::dto::CreatedApiToken,
        ferry_core::dto::StartCliLogin,
        ferry_core::dto::CliLoginStarted,
        ferry_core::dto::CliLoginStatus,
        ferry_core::dto::CliLoginView,
        ferry_core::dto::CliLoginPoll,
        ferry_core::dto::CliLoginResult,
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
        ferry_core::dto::AuthorizeGit,
        ferry_core::dto::GitCallback,
        ferry_core::dto::GitAuthorization,
        ferry_core::dto::GitAuthorizationStatus,
        ferry_core::dto::GitRedirectMethod,
        ferry_core::dto::ConnectGit,
        ferry_core::dto::GitConnectionView,
        ferry_core::dto::GitConnectionStatus,
        ferry_core::GitAuth,
        ferry_core::dto::GitRepository,
        ferry_core::dto::GitRepositoryList,
        ferry_core::dto::GitBranches,
        ferry_core::dto::ConnectDomain,
        ferry_core::dto::UpdateDomain,
        ferry_core::dto::DomainView,
        ferry_core::dto::DnsRecord,
        ferry_core::dto::CertificateView,
        ferry_core::dto::CertificateState,
        ferry_core::Domain,
        ferry_core::DomainSource,
        ferry_core::DomainStatus,
        ferry_core::DomainCheck,
        ferry_core::DomainCheckKind,
        ferry_core::CheckOutcome,
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
        ferry_core::GitProvider,
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
        (name = "auth", description = "The account of the server (its administrator), signing in and out of the dashboard, the sessions and the API tokens, and `ferry login` (a terminal asks, the dashboard approves). The status, the first-run setup, signing in and out, and the two calls of a terminal need no authentication; the others only accept the dashboard's session, never an API token."),
        (name = "services", description = "Web services, private services, background workers, static sites and cron jobs: CRUD, lifecycle actions (restart, suspend, resume, scale, rollback), status and runtime logs."),
        (name = "deploys", description = "Deploy history, manual deploys, source uploads (`ferry up`), cancellation and build logs."),
        (name = "env", description = "A service's own environment variables. `?restart=true` restarts the live service when its effective environment changed."),
        (name = "env-groups", description = "Shared variable sets linked to services (a service's own variables win over its groups'), and linking / unlinking them."),
        (name = "domains", description = "The domains services are served under — `<service>.<domain>` for every web service and static site: the server's base domain and the domains connected to it, each pointed at the server with one wildcard DNS record and verified by the server — the certificates of the routed hostnames, and the custom domains of one service (unique across services)."),
        (name = "jobs", description = "Job runs: cron runs and one-off commands, with their logs."),
        (name = "datastores", description = "Managed Postgres and Redis instances with their connection strings and resource limits."),
        (name = "git", description = "Git connections: the GitHub / GitLab accounts this server is authorized to read the repositories of, to pick a repository and a branch from a list and to clone private repositories. An account is authorized in the browser (a GitHub App on GitHub, an OAuth application on GitLab) or with an access token. Connections belong to the server, not to a service: a repository is cloned with the connection that serves its URL."),
        (name = "blueprints", description = "Infrastructure as code: apply a `ferry.yaml` / `render.yaml` (idempotent, with a dry run)."),
        (name = "events", description = "The change feed (Server-Sent Events) the web client uses to stay current without polling."),
        (name = "hooks", description = "Webhooks: secret deploy hook URLs and GitHub push events. They authenticate with their own secrets, never the API token."),
    )
)]
pub struct ApiDoc;

/// Marks every `/api/v1` operation with what authenticates it — an API
/// token or the dashboard's session; the session alone under
/// [`SESSION_PATHS_PREFIX`]; nothing for [`PUBLIC_PATHS`] — and adds the
/// responses they share: 401, 5XX (webhooks too) and, for operations with a
/// body or query parameters, 400.
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
                        "An API token: one created in the dashboard or by `ferry login`, or the server token \
                         stored in `<data-dir>/api_token`. `GET` requests may send it as `?access_token=` \
                         instead.",
                    ))
                    .build(),
            ),
        );
        components.add_security_scheme(
            SESSION_SCHEME,
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                crate::auth::SESSION_COOKIE,
                "The dashboard's session: the cookie `POST /api/v1/auth/login` sets, named `ferry_session` \
                 followed by the port of the address the browser uses (`ferry_session_7878`). A request that \
                 changes something must also come from the dashboard's own origin.",
            ))),
        );
        for (path, item) in openapi.paths.paths.iter_mut() {
            let api = path.starts_with("/api/v1/");
            let authenticated = api && !PUBLIC_PATHS.contains(&path.as_str());
            let session_only = authenticated && path.starts_with(SESSION_PATHS_PREFIX);
            let fallible = api || path.starts_with("/hooks/");
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
                    let none = Vec::<String>::new;
                    let session = SecurityRequirement::new(SESSION_SCHEME, none());
                    op.security = Some(if session_only {
                        vec![session]
                    } else {
                        vec![SecurityRequirement::new(SECURITY_SCHEME, none()), session]
                    });
                    add_response(op, "401", "Missing or invalid API token, or no session.");
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
    GitConnection,
    Domain,
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

/// The OpenAPI document as pretty JSON (used by `ferryd --dump-openapi` to
/// feed the documentation site generator).
pub fn document_json() -> String {
    ApiDoc::openapi().to_pretty_json().unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

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
