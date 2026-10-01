//! `/api/v1/auth` — the account of the server, its sessions and API tokens,
//! and `ferry login` (DESIGN.md §20).
//!
//! Public: the status, the first-run setup, signing in and out, and the two
//! calls a terminal makes for `ferry login`. Everything else needs the
//! dashboard's session: an API token can't change the account or make
//! other tokens.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use ferry_core::dto::{
    ApiErrorBody, ApiTokenView, AuthKind, AuthStatus, ChangePassword, CliLoginPoll, CliLoginResult, CliLoginStarted,
    CliLoginStatus, CliLoginView, CreateApiToken, CreatedApiToken, Login, SessionView, SetupAccount, StartCliLogin,
    UserView,
};
use ferry_core::{ApiToken, Error, Session, User, auth as accounts};
use http::{HeaderMap, Method, StatusCode, Uri, header};

use crate::auth::{self, CliPoll, Identity, Runtime};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath};
use crate::{AppState, setup};

/// Longest name of an API token or of a `ferry login` request.
const MAX_NAME_LEN: usize = 100;

/// Password hashing is slow on purpose: keep it off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| Error::internal(format!("password hashing task: {e}")).into())
}

/// Sign-in and setup requests carry the browser's cookies to come: they must
/// come from the dashboard too.
fn require_same_origin(headers: &HeaderMap) -> ApiResult<()> {
    if auth::same_origin(headers) {
        Ok(())
    } else {
        Err(ApiError::forbidden("cross_site_request", "this request doesn't come from the dashboard"))
    }
}

/// A trimmed name of 1 to 100 characters.
fn clean_name(name: &str, what: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.contains(char::is_control) {
        return Err(ApiError::bad_request(format!("{what} must have 1 to {MAX_NAME_LEN} characters")));
    }
    Ok(name.to_string())
}

/// Start a session for `user` and answer with its cookie.
async fn sign_in(st: &AppState, user: &User, headers: &HeaderMap, status: StatusCode) -> ApiResult<Response> {
    let secret = accounts::new_session_secret();
    let user_agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    st.store.create_session(&Session::new(&user.id, &secret, user_agent)).await?;
    // Sessions nobody uses any more end here, where one is written anyway.
    st.store.delete_expired_sessions().await?;
    let body = AuthStatus { setup_required: false, auth: Some(AuthKind::Session), user: Some(UserView::from(user)) };
    Ok((status, [(header::SET_COOKIE, auth::session_cookie(&secret, headers))], Json(body)).into_response())
}

/// `GET /api/v1/auth/status`
#[utoipa::path(
    get,
    path = "/api/v1/auth/status",
    tag = "auth",
    operation_id = "getAuthStatus",
    summary = "Is there an account, and is this request signed in",
    description = "What the dashboard asks first. `setup_required` is true while the server has no account: it is then created with `POST /api/v1/auth/setup`. `auth` says how the request is authenticated (`session` for the dashboard's cookie, `token` for an API token), or is `null` when it isn't; credentials that are wrong or expired count as none. No token needed.",
    responses((status = 200, description = "The status.", body = AuthStatus)),
)]
pub async fn status(
    State(st): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<Json<AuthStatus>> {
    let setup_required = st.store.first_user().await?.is_none();
    let identity = auth::authenticate(&st, &method, &uri, &headers).await.ok().flatten();
    let (auth, user) = match &identity {
        Some(Identity::Session { user, .. }) => (Some(AuthKind::Session), Some(UserView::from(user.as_ref()))),
        Some(Identity::ApiToken | Identity::ServerToken) => (Some(AuthKind::Token), None),
        None => (None, None),
    };
    Ok(Json(AuthStatus { setup_required, auth, user }))
}

/// `POST /api/v1/auth/setup`
#[utoipa::path(
    post,
    path = "/api/v1/auth/setup",
    tag = "auth",
    operation_id = "setupAccount",
    summary = "Create the administrator's account",
    description = "Only while the server has no account (`setup_required`). `code` is the setup code: `ferryd` prints it in a link (`/setup?code=…`) when it starts without an account, and keeps it in `<data-dir>/setup_code`, so only someone on the server can create the account. The answer signs the browser in (it sets the session cookie). No token needed.",
    request_body = SetupAccount,
    responses(
        (status = 201, description = "The account was created and this browser is signed in.", body = AuthStatus),
        (status = 400, description = "Invalid email, or a password shorter than 8 characters.", body = ApiErrorBody),
        (status = 403, description = "Wrong setup code (code `invalid_setup_code`).", body = ApiErrorBody),
        (status = 409, description = "The server already has an account.", body = ApiErrorBody),
        (status = 429, description = "Too many failed attempts (code `too_many_attempts`).", body = ApiErrorBody),
    ),
)]
pub async fn setup(
    State(st): State<AppState>,
    Extension(runtime): Extension<Arc<Runtime>>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<SetupAccount>,
) -> ApiResult<Response> {
    require_same_origin(&headers)?;
    let exists = || Error::conflict("this server already has an account: sign in instead");
    if st.store.first_user().await?.is_some() {
        return Err(exists().into());
    }
    runtime.check_attempts()?;
    // Fail closed: without a code on disk nobody can set the server up.
    let expected = setup::ensure_code(&st.config, &st.store).await?.unwrap_or_default();
    if expected.is_empty() || !auth::secrets_equal(req.code.trim(), &expected) {
        runtime.record_failure();
        return Err(ApiError::forbidden(
            "invalid_setup_code",
            "wrong setup code: open the link `ferryd status` prints on the server, or copy the code from <data-dir>/setup_code",
        ));
    }
    let email = accounts::normalize_email(&req.email)?;
    accounts::validate_password(&req.password)?;
    let password = req.password;
    let hash = blocking(move || accounts::hash_password(&password)).await??;
    let user = User::new(&email, &hash);
    if !st.store.create_first_user(&user).await? {
        return Err(exists().into());
    }
    setup::clear_code(&st.config.data_dir);
    tracing::info!(email = %user.email, "the administrator's account was created");
    sign_in(&st, &user, &headers, StatusCode::CREATED).await
}

/// `POST /api/v1/auth/login`
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    operation_id = "login",
    summary = "Sign in to the dashboard",
    description = "Checks the email and password of the account and starts a session: the answer sets the session cookie (`HttpOnly`, `SameSite=Strict`; named `ferry_session_<port>` after the port of the address the browser uses, so that servers sharing a host name keep a session each), which authenticates the dashboard's requests from then on. A session ends 30 days after it was last used. After 10 failed attempts in 5 minutes, sign-ins are refused until the oldest is 5 minutes old. No token needed.",
    request_body = Login,
    responses(
        (status = 200, description = "Signed in.", body = AuthStatus),
        (status = 401, description = "Wrong email or password (code `invalid_credentials`).", body = ApiErrorBody),
        (status = 429, description = "Too many failed attempts (code `too_many_attempts`).", body = ApiErrorBody),
    ),
)]
pub async fn login(
    State(st): State<AppState>,
    Extension(runtime): Extension<Arc<Runtime>>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<Login>,
) -> ApiResult<Response> {
    require_same_origin(&headers)?;
    runtime.check_attempts()?;
    let user = match accounts::normalize_email(&req.email) {
        Ok(email) => st.store.find_user_by_email(&email).await?,
        Err(_) => None,
    };
    // An unknown email takes as long to refuse as a wrong password.
    let hash = user.as_ref().map_or_else(|| auth::dummy_password_hash().to_string(), |u| u.password_hash.clone());
    let password = req.password;
    let matches = blocking(move || accounts::verify_password(&password, &hash)).await?;
    match user {
        Some(user) if matches => sign_in(&st, &user, &headers, StatusCode::OK).await,
        _ => {
            runtime.record_failure();
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "wrong email or password"))
        }
    }
}

/// `POST /api/v1/auth/logout`
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    operation_id = "logout",
    summary = "Sign out",
    description = "Ends the session of the request's cookie and clears the cookie. Succeeds without a session too.",
    responses((status = 204, description = "Signed out.")),
)]
pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    require_same_origin(&headers)?;
    if let Some(secret) = auth::session_secret(&headers)
        && let Some(session) = st.store.find_session(&accounts::digest(secret)).await?
    {
        st.store.delete_session(&session.id).await?;
    }
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, auth::clear_session_cookie(&headers))]).into_response())
}

/// `POST /api/v1/auth/password`
#[utoipa::path(
    post,
    path = "/api/v1/auth/password",
    tag = "auth",
    operation_id = "changePassword",
    summary = "Change the password",
    description = "Needs the dashboard's session and the current password. Every other session of the account ends; this one stays. (A forgotten password is replaced on the server with `ferryd reset-password`.)",
    request_body = ChangePassword,
    responses(
        (status = 204, description = "Changed."),
        (status = 400, description = "The new password is shorter than 8 characters.", body = ApiErrorBody),
        (status = 401, description = "Not signed in, or the current password is wrong (code `invalid_credentials`).", body = ApiErrorBody),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
        (status = 429, description = "Too many failed attempts (code `too_many_attempts`).", body = ApiErrorBody),
    ),
)]
pub async fn change_password(
    State(st): State<AppState>,
    Extension(runtime): Extension<Arc<Runtime>>,
    Extension(identity): Extension<Identity>,
    ApiJson(req): ApiJson<ChangePassword>,
) -> ApiResult<StatusCode> {
    let (session, user) = identity.session()?;
    runtime.check_attempts()?;
    accounts::validate_password(&req.new_password)?;
    let (current, hash) = (req.current_password, user.password_hash.clone());
    if !blocking(move || accounts::verify_password(&current, &hash)).await? {
        runtime.record_failure();
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "the current password is wrong"));
    }
    let new = req.new_password;
    let hash = blocking(move || accounts::hash_password(&new)).await??;
    st.store.set_password(&user.id, &hash).await?;
    st.store.delete_sessions(&user.id, Some(&session.id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/auth/sessions`
#[utoipa::path(
    get,
    path = "/api/v1/auth/sessions",
    tag = "auth",
    operation_id = "listSessions",
    summary = "List the sessions",
    description = "The browsers signed in to the account, the last used first; `current` marks the one of this request. Needs the dashboard's session.",
    responses(
        (status = 200, description = "The sessions.", body = Vec<SessionView>),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
    ),
)]
pub async fn list_sessions(
    State(st): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Json<Vec<SessionView>>> {
    let (current, user) = identity.session()?;
    let sessions = st.store.list_sessions(&user.id).await?;
    Ok(Json(
        sessions
            .into_iter()
            .map(|s| SessionView {
                current: s.id == current.id,
                id: s.id,
                user_agent: s.user_agent,
                created_at: s.created_at,
                last_used_at: s.last_used_at,
                expires_at: s.expires_at,
            })
            .collect(),
    ))
}

/// `DELETE /api/v1/auth/sessions/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/auth/sessions/{id}",
    tag = "auth",
    operation_id = "deleteSession",
    summary = "End a session",
    description = "Signs that browser out. Ending the session of this request is signing out. Needs the dashboard's session.",
    params(("id" = String, Path, description = "Session id.")),
    responses(
        (status = 204, description = "Ended."),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
        (status = 404, description = "No such session.", body = ApiErrorBody),
    ),
)]
pub async fn delete_session(
    State(st): State<AppState>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Response> {
    let (current, user) = identity.session()?;
    let id = id.trim();
    let owned = st.store.list_sessions(&user.id).await?.iter().any(|s| s.id == id);
    if !owned || !st.store.delete_session(id).await? {
        return Err(Error::not_found("session", id).into());
    }
    if id == current.id {
        let cleared = auth::clear_session_cookie(&headers);
        return Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cleared)]).into_response());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/v1/auth/tokens`
#[utoipa::path(
    get,
    path = "/api/v1/auth/tokens",
    tag = "auth",
    operation_id = "listApiTokens",
    summary = "List the API tokens",
    description = "The named API tokens, the newest first: their name, the end of the token and when they were last used. Never the tokens themselves. The server token of `<data-dir>/api_token` isn't listed. Needs the dashboard's session.",
    responses(
        (status = 200, description = "The API tokens.", body = Vec<ApiTokenView>),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
    ),
)]
pub async fn list_tokens(
    State(st): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Json<Vec<ApiTokenView>>> {
    identity.session()?;
    Ok(Json(st.store.list_api_tokens().await?.iter().map(ApiTokenView::from).collect()))
}

/// When a token made now with `expires_in_days` ends.
fn token_expiry(expires_in_days: Option<u32>) -> ApiResult<Option<chrono::DateTime<Utc>>> {
    match expires_in_days {
        None => Ok(None),
        Some(days @ 1..=3650) => Ok(Some(ferry_core::now() + chrono::Duration::days(i64::from(days)))),
        Some(_) => Err(ApiError::bad_request("expires_in_days must be between 1 and 3650")),
    }
}

/// `POST /api/v1/auth/tokens`
#[utoipa::path(
    post,
    path = "/api/v1/auth/tokens",
    tag = "auth",
    operation_id = "createApiToken",
    summary = "Create an API token",
    description = "A token for the CLI, a script or CI: it authenticates as `Authorization: Bearer <token>` and can do everything the API offers except manage the account and its tokens. The answer is the only time the token is shown; the server keeps its SHA-256. Needs the dashboard's session.",
    request_body = CreateApiToken,
    responses(
        (status = 201, description = "The token, shown once.", body = CreatedApiToken),
        (status = 400, description = "Invalid name or `expires_in_days`.", body = ApiErrorBody),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
    ),
)]
pub async fn create_token(
    State(st): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiJson(req): ApiJson<CreateApiToken>,
) -> ApiResult<(StatusCode, Json<CreatedApiToken>)> {
    identity.session()?;
    let name = clean_name(&req.name, "the name of a token")?;
    let token = accounts::new_api_token();
    let api_token = ApiToken::new(&name, &token, token_expiry(req.expires_in_days)?);
    st.store.create_api_token(&api_token).await?;
    Ok((StatusCode::CREATED, Json(CreatedApiToken { token, api_token: ApiTokenView::from(&api_token) })))
}

/// `DELETE /api/v1/auth/tokens/{id}`
#[utoipa::path(
    delete,
    path = "/api/v1/auth/tokens/{id}",
    tag = "auth",
    operation_id = "deleteApiToken",
    summary = "Revoke an API token",
    description = "The token stops working at once. Needs the dashboard's session.",
    params(("id" = String, Path, description = "API token id (`tok-…`).")),
    responses(
        (status = 204, description = "Revoked."),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
        (status = 404, description = "No such API token.", body = ApiErrorBody),
    ),
)]
pub async fn delete_token(
    State(st): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<StatusCode> {
    identity.session()?;
    st.store.delete_api_token(id.trim()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The error of a `ferry login` request that is gone.
fn cli_login_gone() -> ApiError {
    ApiError::not_found("this login request has expired or was already answered: run 'ferry login' again")
}

/// `POST /api/v1/auth/cli`
#[utoipa::path(
    post,
    path = "/api/v1/auth/cli",
    tag = "auth",
    operation_id = "startCliLogin",
    summary = "Start a `ferry login`",
    description = "What `ferry login` calls first. The terminal then shows `code` and opens `/cli-login?id=<id>` of the dashboard, where the signed-in administrator checks the code and approves; meanwhile the terminal polls `POST /api/v1/auth/cli/{id}/token` with `secret`. A request waits 10 minutes, and a restart of the server forgets it. No token needed.",
    request_body = StartCliLogin,
    responses(
        (status = 201, description = "The request.", body = CliLoginStarted),
        (status = 409, description = "The server has no account yet: set it up first.", body = ApiErrorBody),
        (status = 429, description = "Too many requests are waiting.", body = ApiErrorBody),
    ),
)]
pub async fn cli_start(
    State(st): State<AppState>,
    Extension(runtime): Extension<Arc<Runtime>>,
    ApiJson(req): ApiJson<StartCliLogin>,
) -> ApiResult<(StatusCode, Json<CliLoginStarted>)> {
    if st.store.first_user().await?.is_none() {
        return Err(Error::conflict(
            "this server has no account yet: create it in the dashboard first (`ferryd status` prints the link)",
        )
        .into());
    }
    // Tokens of approvals no terminal came back for.
    for id in runtime.expired_cli_tokens() {
        let _ = st.store.delete_api_token(&id).await;
    }
    let name = match req.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => clean_name(name, "the name of the terminal")?,
        None => "ferry CLI".to_string(),
    };
    let (id, code, secret) = runtime.start_cli_login(&name)?;
    let started = CliLoginStarted {
        id,
        code,
        secret,
        expires_in: auth::CLI_LOGIN_TTL_SECS,
        interval: auth::CLI_LOGIN_INTERVAL_SECS,
    };
    Ok((StatusCode::CREATED, Json(started)))
}

/// `GET /api/v1/auth/cli/{id}`
#[utoipa::path(
    get,
    path = "/api/v1/auth/cli/{id}",
    tag = "auth",
    operation_id = "getCliLogin",
    summary = "Show a `ferry login` request",
    description = "What the approval page shows: who asks and the code the terminal displays. Needs the dashboard's session.",
    params(("id" = String, Path, description = "Id of the request.")),
    responses(
        (status = 200, description = "The request.", body = CliLoginView),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
        (status = 404, description = "Unknown, expired or already collected.", body = ApiErrorBody),
    ),
)]
pub async fn cli_get(
    Extension(runtime): Extension<Arc<Runtime>>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<CliLoginView>> {
    identity.session()?;
    runtime.cli_login(id.trim()).map(Json).ok_or_else(cli_login_gone)
}

/// `POST /api/v1/auth/cli/{id}/approve`
#[utoipa::path(
    post,
    path = "/api/v1/auth/cli/{id}/approve",
    tag = "auth",
    operation_id = "approveCliLogin",
    summary = "Approve a `ferry login`",
    description = "Creates an API token named after the request and hands it to the terminal that asked, the next time it polls. Approve only when the code on the page is the one your terminal shows. Needs the dashboard's session.",
    params(("id" = String, Path, description = "Id of the request.")),
    responses(
        (status = 200, description = "Approved.", body = CliLoginView),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
        (status = 404, description = "Unknown, expired or already answered.", body = ApiErrorBody),
    ),
)]
pub async fn cli_approve(
    State(st): State<AppState>,
    Extension(runtime): Extension<Arc<Runtime>>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<CliLoginView>> {
    identity.session()?;
    let id = id.trim();
    let pending = runtime.cli_login(id).filter(|r| r.status == CliLoginStatus::Pending).ok_or_else(cli_login_gone)?;
    let token = accounts::new_api_token();
    let api_token = ApiToken::new(&pending.name, &token, None);
    st.store.create_api_token(&api_token).await?;
    match runtime.answer_cli_login(id, Some((api_token.id.clone(), token))) {
        Some(view) => Ok(Json(view)),
        None => {
            // Answered or expired meanwhile: nobody will collect this token.
            st.store.delete_api_token(&api_token.id).await?;
            Err(cli_login_gone())
        }
    }
}

/// `POST /api/v1/auth/cli/{id}/deny`
#[utoipa::path(
    post,
    path = "/api/v1/auth/cli/{id}/deny",
    tag = "auth",
    operation_id = "denyCliLogin",
    summary = "Deny a `ferry login`",
    description = "The terminal that asked is told so the next time it polls. Needs the dashboard's session.",
    params(("id" = String, Path, description = "Id of the request.")),
    responses(
        (status = 200, description = "Denied.", body = CliLoginView),
        (status = 403, description = "Authenticated with an API token (code `session_required`).", body = ApiErrorBody),
        (status = 404, description = "Unknown, expired or already answered.", body = ApiErrorBody),
    ),
)]
pub async fn cli_deny(
    Extension(runtime): Extension<Arc<Runtime>>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<CliLoginView>> {
    identity.session()?;
    runtime.answer_cli_login(id.trim(), None).map(Json).ok_or_else(cli_login_gone)
}

/// `POST /api/v1/auth/cli/{id}/token`
#[utoipa::path(
    post,
    path = "/api/v1/auth/cli/{id}/token",
    tag = "auth",
    operation_id = "pollCliLogin",
    summary = "Collect the token of a `ferry login`",
    description = "What the terminal polls, every `interval` seconds, with the `secret` it got when it started the request. `pending` until the request is answered; `approved` comes with the API token, once: the request is gone afterwards, like after `denied`. No token needed.",
    params(("id" = String, Path, description = "Id of the request.")),
    request_body = CliLoginPoll,
    responses(
        (status = 200, description = "Where the request stands.", body = CliLoginResult),
        (status = 401, description = "Not the secret of this request.", body = ApiErrorBody),
        (status = 404, description = "Unknown, expired or already collected.", body = ApiErrorBody),
    ),
)]
pub async fn cli_token(
    Extension(runtime): Extension<Arc<Runtime>>,
    ApiPath(id): ApiPath<String>,
    ApiJson(req): ApiJson<CliLoginPoll>,
) -> ApiResult<Json<CliLoginResult>> {
    let result = match runtime.poll_cli_login(id.trim(), &req.secret)?.ok_or_else(cli_login_gone)? {
        CliPoll::Pending => CliLoginResult { status: CliLoginStatus::Pending, token: None },
        CliPoll::Denied => CliLoginResult { status: CliLoginStatus::Denied, token: None },
        CliPoll::Approved(token) => CliLoginResult { status: CliLoginStatus::Approved, token: Some(token) },
    };
    Ok(Json(result))
}
