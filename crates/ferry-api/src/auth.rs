//! Authentication of `/api/*` (DESIGN.md §20).
//!
//! A request is authenticated by one of:
//! * `Authorization: Bearer <token>` (or `?access_token=` on `GET`): the
//!   server token of `<data-dir>/api_token`, or a named API token;
//! * the session cookie the dashboard gets when it signs in. A cookie is
//!   sent by the browser on its own, so a request that changes something
//!   must also come from the dashboard's own origin ([`same_origin`]).
//!
//! [`Runtime`] holds what lives in memory only: the failed sign-ins that
//! are counted against brute force, and the pending `ferry login` requests.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use ferry_core::dto::{CliLoginStatus, CliLoginView};
use ferry_core::{Session, User};
use http::{HeaderMap, HeaderValue, Method, Uri, header};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::AppState;
use crate::error::ApiError;

/// Name of the session cookie, before the port the browser uses (see
/// [`session_cookie_name`]).
pub const SESSION_COOKIE: &str = "ferry_session";
/// How long a browser keeps the cookie. The server decides when the session
/// ends (30 days after its last use), so this is only an upper bound.
const COOKIE_MAX_AGE_SECS: u64 = 400 * 24 * 3600;
/// A session or token records that it was used at most this often.
const TOUCH_EVERY: chrono::Duration = chrono::Duration::minutes(5);
/// Failed sign-ins (and setups) allowed per [`FAILURE_WINDOW`], for the whole server.
const MAX_FAILURES: usize = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(5 * 60);
/// A `ferry login` request waits this long for its approval.
pub const CLI_LOGIN_TTL_SECS: u64 = 10 * 60;
/// Seconds `ferry login` waits between two polls.
pub const CLI_LOGIN_INTERVAL_SECS: u64 = 2;
/// Pending `ferry login` requests kept at once.
const MAX_CLI_LOGINS: usize = 100;

/// Compare two secrets in constant time. Both sides are hashed first so that
/// neither the contents nor the length of the expected secret leak through
/// timing.
pub fn secrets_equal(provided: &str, expected: &str) -> bool {
    let a = Sha256::digest(provided.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    bool::from(a.as_slice().ct_eq(b.as_slice()))
}

/// Who a request is.
#[derive(Debug, Clone)]
pub enum Identity {
    /// The dashboard, signed in to the account.
    Session { session: Box<Session>, user: Box<User> },
    /// A named API token.
    ApiToken,
    /// The server token (`<data-dir>/api_token`, `--api-token`).
    ServerToken,
}

impl Identity {
    /// The session and its account, for what only the signed-in dashboard
    /// may do (the account itself, its tokens): 403 for an API token.
    pub fn session(&self) -> Result<(&Session, &User), ApiError> {
        match self {
            Identity::Session { session, user } => Ok((session, user)),
            _ => Err(ApiError::forbidden(
                "session_required",
                "this is only possible from the dashboard, signed in to the account (not with an API token)",
            )),
        }
    }
}

/// Token from `Authorization: Bearer <token>`.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_string())
}

/// Token from `?access_token=` (GET only: EventSource can't set headers).
fn query_token(method: &Method, uri: &Uri) -> Option<String> {
    if method != Method::GET {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Q {
        access_token: Option<String>,
    }
    let axum::extract::Query(q) = axum::extract::Query::<Q>::try_from_uri(uri).ok()?;
    q.access_token
}

/// The value of a cookie of the request.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// The first value of a header, trimmed.
fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim).filter(|v| !v.is_empty())
}

/// Was the request made by the dashboard's own pages? What a browser sends
/// along with a cookie decides: `Sec-Fetch-Site`, else the `Origin`, which
/// must be the host the request was sent to. A client that sends neither
/// isn't a browser, and nothing made it send the cookie.
pub fn same_origin(headers: &HeaderMap) -> bool {
    if let Some(site) = header_str(headers, "sec-fetch-site") {
        return site == "same-origin";
    }
    let Some(origin) = header_str(headers, "origin") else { return true };
    let Some((_, authority)) = origin.split_once("://") else { return false };
    // Behind a proxy, the host the browser used is the forwarded one.
    let hosts = [header_str(headers, "x-forwarded-host"), header_str(headers, "host")];
    hosts.into_iter().flatten().any(|host| host.eq_ignore_ascii_case(authority))
}

/// Did the browser reach the server over HTTPS (through a proxy that says
/// so)? Then the session cookie is only ever sent over HTTPS.
fn is_https(headers: &HeaderMap) -> bool {
    header_str(headers, "x-forwarded-proto").is_some_and(|proto| proto.eq_ignore_ascii_case("https"))
}

/// The name of the session cookie for the server this request reached:
/// `ferry_session_<port>`, the port being the one the browser used (of the
/// forwarded host behind a proxy), or `ferry_session` when the address has
/// none. Browsers keep cookies per host name, not per port: two servers
/// reached under one name — `127.0.0.1:7878` and `127.0.0.1:7879`, or two
/// SSH tunnels — would otherwise overwrite each other's session.
pub fn session_cookie_name(headers: &HeaderMap) -> String {
    let host = header_str(headers, "x-forwarded-host").or_else(|| header_str(headers, "host")).unwrap_or_default();
    let port = host.rsplit_once(':').map(|(_, port)| port);
    match port.filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        Some(port) => format!("{SESSION_COOKIE}_{port}"),
        None => SESSION_COOKIE.to_string(),
    }
}

/// The session secret this request carries, if any.
pub fn session_secret(headers: &HeaderMap) -> Option<&str> {
    cookie(headers, &session_cookie_name(headers))
}

/// `Set-Cookie` that gives the browser its session.
pub fn session_cookie(secret: &str, headers: &HeaderMap) -> HeaderValue {
    let name = session_cookie_name(headers);
    let secure = if is_https(headers) { "; Secure" } else { "" };
    let cookie = format!("{name}={secret}; Path=/; HttpOnly; SameSite=Strict; Max-Age={COOKIE_MAX_AGE_SECS}{secure}");
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// `Set-Cookie` that takes the session away.
pub fn clear_session_cookie(headers: &HeaderMap) -> HeaderValue {
    let cookie = format!("{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0", session_cookie_name(headers));
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// A `ferry login` waiting for its approval in the dashboard.
struct CliLogin {
    name: String,
    code: String,
    /// Digest of the secret the terminal polls with.
    secret_hash: String,
    status: CliLoginStatus,
    /// The API token an approval made: its id, and the token until the
    /// terminal collects it.
    token: Option<(String, String)>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl CliLogin {
    fn view(&self, id: &str) -> CliLoginView {
        CliLoginView {
            id: id.to_string(),
            name: self.name.clone(),
            code: self.code.clone(),
            status: self.status,
            created_at: self.created_at,
            expires_at: self.expires_at,
        }
    }
}

/// What the terminal learns when it polls.
pub enum CliPoll {
    Pending,
    Denied,
    /// The token, handed over once.
    Approved(String),
}

/// The in-memory side of authentication. A restart of the server forgets it:
/// a pending `ferry login` is started again.
#[derive(Default)]
pub struct Runtime {
    failures: Mutex<VecDeque<Instant>>,
    cli: Mutex<HashMap<String, CliLogin>>,
}

impl Runtime {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Refuse a sign-in while too many failed recently.
    pub fn check_attempts(&self) -> Result<(), ApiError> {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while failures.front().is_some_and(|at| now.duration_since(*at) > FAILURE_WINDOW) {
            failures.pop_front();
        }
        if failures.len() < MAX_FAILURES {
            return Ok(());
        }
        let wait = failures.front().map_or(FAILURE_WINDOW, |at| FAILURE_WINDOW.saturating_sub(now.duration_since(*at)));
        Err(ApiError::too_many_attempts(wait.as_secs().max(1)))
    }

    pub fn record_failure(&self) {
        self.failures.lock().unwrap_or_else(|e| e.into_inner()).push_back(Instant::now());
    }

    /// Start a `ferry login`: `(id, code, secret)`. `Err` when too many wait.
    pub fn start_cli_login(&self, name: &str) -> Result<(String, String, String), ApiError> {
        let now = Utc::now();
        let mut cli = self.cli.lock().unwrap_or_else(|e| e.into_inner());
        // (A token of an approval nobody collected is revoked by `expired_cli_tokens`.)
        cli.retain(|_, r| r.expires_at > now || r.token.is_some());
        if cli.len() >= MAX_CLI_LOGINS {
            return Err(ApiError::too_many_requests(
                "too_many_requests",
                "too many 'ferry login' requests are waiting: try again in a few minutes",
            ));
        }
        let id = ferry_core::ids::random_secret(24);
        let code = cli_code();
        let secret = ferry_core::ids::random_secret(40);
        cli.insert(
            id.clone(),
            CliLogin {
                name: name.to_string(),
                code: code.clone(),
                secret_hash: ferry_core::auth::digest(&secret),
                status: CliLoginStatus::Pending,
                token: None,
                created_at: now,
                expires_at: now + chrono::Duration::seconds(CLI_LOGIN_TTL_SECS as i64),
            },
        );
        Ok((id, code, secret))
    }

    /// A request that hasn't expired.
    pub fn cli_login(&self, id: &str) -> Option<CliLoginView> {
        let cli = self.cli.lock().unwrap_or_else(|e| e.into_inner());
        cli.get(id).filter(|r| r.expires_at > Utc::now()).map(|r| r.view(id))
    }

    /// Answer a pending request: `approved` with the id and value of the
    /// token made for it, or `None` to deny. `None` when it is unknown,
    /// expired or already answered.
    pub fn answer_cli_login(&self, id: &str, approved: Option<(String, String)>) -> Option<CliLoginView> {
        let mut cli = self.cli.lock().unwrap_or_else(|e| e.into_inner());
        let request = cli.get_mut(id).filter(|r| r.expires_at > Utc::now() && r.status == CliLoginStatus::Pending)?;
        request.status = if approved.is_some() { CliLoginStatus::Approved } else { CliLoginStatus::Denied };
        request.token = approved;
        Some(request.view(id))
    }

    /// What the terminal that started `id` gets. `None`: unknown, expired or
    /// already collected. `Err`: not the terminal that started it.
    pub fn poll_cli_login(&self, id: &str, secret: &str) -> Result<Option<CliPoll>, ApiError> {
        let mut cli = self.cli.lock().unwrap_or_else(|e| e.into_inner());
        let Some(request) = cli.get(id).filter(|r| r.expires_at > Utc::now()) else { return Ok(None) };
        if !secrets_equal(&ferry_core::auth::digest(secret), &request.secret_hash) {
            return Err(ApiError::unauthorized("this login request was started by another terminal"));
        }
        Ok(Some(match request.status {
            CliLoginStatus::Pending => CliPoll::Pending,
            CliLoginStatus::Denied => {
                cli.remove(id);
                CliPoll::Denied
            }
            CliLoginStatus::Approved => match cli.remove(id).and_then(|r| r.token) {
                Some((_, token)) => CliPoll::Approved(token),
                None => return Ok(None),
            },
        }))
    }

    /// Ids of the API tokens made for approvals that expired before their
    /// terminal collected them: nobody has those tokens, they are revoked.
    pub fn expired_cli_tokens(&self) -> Vec<String> {
        let now = Utc::now();
        let mut cli = self.cli.lock().unwrap_or_else(|e| e.into_inner());
        let expired: Vec<String> = cli.iter().filter(|(_, r)| r.expires_at <= now).map(|(id, _)| id.clone()).collect();
        expired.iter().filter_map(|id| cli.remove(id)).filter_map(|r| r.token.map(|(id, _)| id)).collect()
    }
}

/// `ABCD-EFGH`: 8 characters of an alphabet without look-alikes (no 0/O, 1/I/L).
fn cli_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let bytes = uuid_bytes();
    let chars: String = bytes.iter().take(8).map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char).collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// 16 random bytes (the hex digits of a random secret, decoded).
fn uuid_bytes() -> Vec<u8> {
    hex::decode(ferry_core::ids::random_secret(32)).unwrap_or_else(|_| vec![0; 16])
}

/// A hash to verify against when no account has the email, so that an
/// unknown email takes as long to refuse as a wrong password.
pub fn dummy_password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| ferry_core::auth::hash_password(&ferry_core::ids::random_secret(32)).unwrap_or_default())
}

/// Who sent this request? `Ok(None)` when it carries no credentials at all;
/// an error when the ones it carries are wrong.
pub async fn authenticate(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Option<Identity>, ApiError> {
    let now = Utc::now();
    if let Some(token) = bearer_token(headers).or_else(|| query_token(method, uri)) {
        let server_token = state.config.api_token.as_str();
        // (An unconfigured server token must never match an empty one.)
        if !server_token.is_empty() && secrets_equal(&token, server_token) {
            return Ok(Some(Identity::ServerToken));
        }
        let found = state.store.find_api_token(&ferry_core::auth::digest(&token)).await?;
        let Some(api_token) = found.filter(|t| !t.is_expired(now)) else {
            return Err(ApiError::unauthorized("invalid API token"));
        };
        if api_token.last_used_at.is_none_or(|at| now - at > TOUCH_EVERY) {
            state.store.touch_api_token(&api_token.id, now).await?;
        }
        return Ok(Some(Identity::ApiToken));
    }
    let Some(secret) = session_secret(headers) else { return Ok(None) };
    let expired = || ApiError::unauthorized("your session has ended: sign in again");
    let session = state.store.find_session(&ferry_core::auth::digest(secret)).await?;
    let Some(session) = session.filter(|s| !s.is_expired(now)) else { return Err(expired()) };
    let Some(user) = state.store.get_user(&session.user_id).await? else { return Err(expired()) };
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe && !same_origin(headers) {
        return Err(ApiError::forbidden("cross_site_request", "this request doesn't come from the dashboard"));
    }
    if now - session.last_used_at > TOUCH_EVERY {
        let expires_at = now + chrono::Duration::days(ferry_core::auth::SESSION_DAYS);
        state.store.touch_session(&session.id, now, expires_at).await?;
    }
    Ok(Some(Identity::Session { session: Box::new(session), user: Box::new(user) }))
}

/// Middleware: require a valid API token or session, and tell the handler
/// who it is (an [`Identity`] extension).
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    match authenticate(&state, req.method(), req.uri(), req.headers()).await {
        Ok(Some(identity)) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        Ok(None) => ApiError::unauthorized(
            "not signed in: send an API token as 'Authorization: Bearer <token>', or sign in to the dashboard",
        )
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// Middleware of the account's routes, inside [`require_auth`]: the
/// dashboard's session, not an API token.
pub async fn require_session(req: Request, next: Next) -> Response {
    match req.extensions().get::<Identity>().map(Identity::session) {
        Some(Ok(_)) => next.run(req).await,
        Some(Err(e)) => e.into_response(),
        None => ApiError::unauthorized("not signed in").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(http::HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn constant_time_compare() {
        assert!(secrets_equal("abc", "abc"));
        assert!(!secrets_equal("abc", "abd"));
        assert!(!secrets_equal("", "abc"));
        assert!(!secrets_equal("abcd", "abc"));
    }

    #[test]
    fn token_sources() {
        assert_eq!(bearer_token(&headers(&[("authorization", "bearer  tok ")])).as_deref(), Some("tok"));
        assert_eq!(bearer_token(&headers(&[("authorization", "Basic xyz")])), None);
        let uri: Uri = "/api/v1/x?follow=1&access_token=a%2Bb".parse().unwrap();
        assert_eq!(query_token(&Method::GET, &uri).as_deref(), Some("a+b"));
        assert_eq!(query_token(&Method::POST, &uri), None);
    }

    #[test]
    fn cookies_are_found_by_name() {
        let h = headers(&[("cookie", "theme=dark; ferry_session=abc; other=1")]);
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&h, "theme"), Some("dark"));
        assert_eq!(cookie(&h, "session"), None, "the whole name");
        let two = headers(&[("cookie", "a=1"), ("cookie", "ferry_session=xyz")]);
        assert_eq!(cookie(&two, SESSION_COOKIE), Some("xyz"));
        assert_eq!(cookie(&headers(&[("cookie", "ferry_session=")]), SESSION_COOKIE), None);
        assert_eq!(cookie(&HeaderMap::new(), SESSION_COOKIE), None);
    }

    #[test]
    fn requests_of_other_sites_are_told_apart() {
        // What a browser says wins.
        assert!(same_origin(&headers(&[("sec-fetch-site", "same-origin")])));
        for site in ["cross-site", "same-site", "none"] {
            assert!(!same_origin(&headers(&[("sec-fetch-site", site), ("host", "ferry.test")])), "{site}");
        }
        // Without it (plain HTTP outside localhost): the origin is the host.
        assert!(same_origin(&headers(&[("origin", "http://10.0.0.5:7878"), ("host", "10.0.0.5:7878")])));
        assert!(same_origin(&headers(&[("origin", "https://Ferry.Example.com"), ("host", "ferry.example.com")])));
        assert!(!same_origin(&headers(&[("origin", "http://evil.test"), ("host", "10.0.0.5:7878")])));
        assert!(!same_origin(&headers(&[("origin", "http://10.0.0.5:9999"), ("host", "10.0.0.5:7878")])));
        assert!(!same_origin(&headers(&[("origin", "null"), ("host", "10.0.0.5:7878")])));
        // Behind a proxy, the host the browser used is the forwarded one.
        let proxied = [
            ("origin", "https://ferry.example.com"),
            ("host", "127.0.0.1:7878"),
            ("x-forwarded-host", "ferry.example.com"),
        ];
        assert!(same_origin(&headers(&proxied)));
        // Not a browser: nothing sent the cookie on its own.
        assert!(same_origin(&headers(&[("host", "127.0.0.1:7878")])));
    }

    #[test]
    fn session_cookies() {
        let plain = session_cookie("fys_abc", &HeaderMap::new());
        assert_eq!(
            plain.to_str().unwrap(),
            "ferry_session=fys_abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000"
        );
        let https = session_cookie("fys_abc", &headers(&[("x-forwarded-proto", "https")]));
        assert!(https.to_str().unwrap().ends_with("; Secure"));
        let cleared = clear_session_cookie(&HeaderMap::new());
        assert_eq!(cleared.to_str().unwrap(), "ferry_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    }

    #[test]
    fn the_session_cookie_is_named_after_the_port() {
        let name = |pairs: &[(&str, &str)]| session_cookie_name(&headers(pairs));
        assert_eq!(name(&[("host", "127.0.0.1:7878")]), "ferry_session_7878");
        assert_eq!(name(&[("host", "[::1]:7879")]), "ferry_session_7879");
        assert_eq!(name(&[("host", "ferry.example.com")]), "ferry_session");
        assert_eq!(name(&[("host", "[::1]")]), "ferry_session");
        assert_eq!(name(&[]), "ferry_session");
        // Behind a proxy: the port the browser used, not the API's own.
        let proxied = [("host", "127.0.0.1:7878"), ("x-forwarded-host", "ferry.localhost:8080")];
        assert_eq!(name(&proxied), "ferry_session_8080");
        assert_eq!(name(&[("host", "127.0.0.1:7878"), ("x-forwarded-host", "ferry.example.com")]), "ferry_session");

        // Two servers under one host name keep a session each.
        let both = "ferry_session_7878=a; ferry_session_7879=b";
        let here = headers(&[("host", "127.0.0.1:7878"), ("cookie", both)]);
        assert_eq!(session_secret(&here), Some("a"));
        assert_eq!(session_secret(&headers(&[("host", "127.0.0.1:7879"), ("cookie", both)])), Some("b"));
        assert_eq!(session_secret(&headers(&[("host", "127.0.0.1:7880"), ("cookie", both)])), None);
        let set = session_cookie("fys_abc", &here);
        assert!(set.to_str().unwrap().starts_with("ferry_session_7878=fys_abc; "), "{set:?}");
        assert!(clear_session_cookie(&here).to_str().unwrap().starts_with("ferry_session_7878=; "));
    }

    #[test]
    fn failed_sign_ins_are_limited() {
        let runtime = Runtime::new();
        for _ in 0..MAX_FAILURES {
            assert!(runtime.check_attempts().is_ok());
            runtime.record_failure();
        }
        let err = runtime.check_attempts().unwrap_err();
        assert_eq!(err.status, http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.body.error.code, "too_many_attempts");
    }

    #[test]
    fn cli_logins_are_answered_once() {
        let runtime = Runtime::new();
        let (id, code, secret) = runtime.start_cli_login("ada@laptop").unwrap();
        assert_eq!(code.len(), 9);
        assert!(code.chars().all(|c| c == '-' || c.is_ascii_uppercase() || c.is_ascii_digit()), "{code}");
        let view = runtime.cli_login(&id).unwrap();
        assert_eq!((view.name.as_str(), view.status), ("ada@laptop", CliLoginStatus::Pending));
        assert!(matches!(runtime.poll_cli_login(&id, &secret), Ok(Some(CliPoll::Pending))));
        // Only the terminal that started it may ask.
        assert!(runtime.poll_cli_login(&id, "another").is_err());
        assert!(matches!(runtime.poll_cli_login("unknown", &secret), Ok(None)));

        let approved = runtime.answer_cli_login(&id, Some(("tok-1".into(), "fy_x".into()))).unwrap();
        assert_eq!(approved.status, CliLoginStatus::Approved);
        assert!(runtime.answer_cli_login(&id, None).is_none(), "answered once");
        assert!(matches!(runtime.poll_cli_login(&id, &secret), Ok(Some(CliPoll::Approved(t))) if t == "fy_x"));
        assert!(matches!(runtime.poll_cli_login(&id, &secret), Ok(None)), "collected once");

        let (id, _, secret) = runtime.start_cli_login("x").unwrap();
        assert_eq!(runtime.answer_cli_login(&id, None).unwrap().status, CliLoginStatus::Denied);
        assert!(matches!(runtime.poll_cli_login(&id, &secret), Ok(Some(CliPoll::Denied))));
        assert!(runtime.cli_login(&id).is_none());
        assert!(runtime.expired_cli_tokens().is_empty());
    }
}
