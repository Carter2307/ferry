//! Accounts (DESIGN.md §20): the first-run setup, signing in and out, what a
//! session may do and from where, API tokens, and `ferry login`.

mod common;

use axum::body::Body;
use common::{Resp, TOKEN, TestApp};
use http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};

const EMAIL: &str = "ada@example.com";

/// A password made up for the test (none is written in this file).
fn new_password() -> String {
    format!("pw-{}", ferry_core::ids::random_secret(16))
}

/// A request as a browser or a terminal makes it: no API token.
struct Call<'a> {
    method: Method,
    uri: &'a str,
    cookie: Option<&'a str>,
    body: Option<Value>,
    headers: Vec<(&'a str, &'a str)>,
}

impl<'a> Call<'a> {
    fn new(method: Method, uri: &'a str) -> Self {
        Call { method, uri, cookie: None, body: None, headers: Vec::new() }
    }

    fn get(uri: &'a str) -> Self {
        Self::new(Method::GET, uri)
    }

    fn post(uri: &'a str, body: Value) -> Self {
        Call { body: Some(body), ..Self::new(Method::POST, uri) }
    }

    fn cookie(mut self, cookie: &'a str) -> Self {
        self.cookie = Some(cookie);
        self
    }

    fn header(mut self, name: &'a str, value: &'a str) -> Self {
        self.headers.push((name, value));
        self
    }

    async fn send(self, app: &TestApp) -> Resp {
        let mut b = Request::builder().method(self.method).uri(self.uri);
        if let Some(cookie) = self.cookie {
            b = b.header(header::COOKIE, cookie);
        }
        for (name, value) in self.headers {
            b = b.header(name, value);
        }
        let body = match self.body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&v).unwrap())
            }
            None => Body::empty(),
        };
        app.send(b.body(body).unwrap()).await
    }
}

/// `ferry_session=<secret>` of an answer that signs in.
fn session_cookie(r: &Resp) -> String {
    let set = r.headers[header::SET_COOKIE].to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

/// Create the account of a fresh server; returns its password and the cookie
/// of the browser that did.
async fn set_up(app: &TestApp) -> (String, String) {
    let code = ferry_api::setup::ensure_code(&app.config, &app.store).await.unwrap().expect("a setup code");
    let password = new_password();
    let r =
        Call::post("/api/v1/auth/setup", json!({"email": EMAIL, "password": password, "code": code})).send(app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let cookie = session_cookie(&r);
    (password, cookie)
}

async fn sign_in(app: &TestApp, password: &str) -> Resp {
    Call::post("/api/v1/auth/login", json!({"email": EMAIL, "password": password})).send(app).await
}

#[tokio::test]
async fn a_new_server_asks_to_be_set_up() {
    let app = TestApp::new().await;
    let r = Call::get("/api/v1/auth/status").send(&app).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!({"setup_required": true, "auth": null, "user": null}));
    // An API token is recognized, with no account.
    let r = app.get("/api/v1/auth/status").await;
    assert_eq!(r.json(), json!({"setup_required": true, "auth": "token", "user": null}));
    // Wrong credentials are no credentials here, not an error.
    let r = Call::get("/api/v1/auth/status").header("authorization", "Bearer nope").send(&app).await;
    assert_eq!(r.json()["auth"], Value::Null);
    // Nobody signs in to a server without an account.
    let r = sign_in(&app, &new_password()).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::UNAUTHORIZED, "invalid_credentials"));
}

#[tokio::test]
async fn setup_takes_the_code_of_the_server_and_signs_in() {
    let app = TestApp::new().await;
    let password = new_password();
    let setup = |code: &str, email: &str, password: &str| {
        Call::post("/api/v1/auth/setup", json!({"email": email, "password": password, "code": code}))
    };

    // Without the code only someone on the server can read: refused.
    let r = setup("guess", EMAIL, &password).send(&app).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::FORBIDDEN, "invalid_setup_code"));
    let r = setup("", EMAIL, &password).send(&app).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(app.store.first_user().await.unwrap().is_none());

    let code = ferry_api::setup::read_code(app.dir.path()).expect("the server wrote a setup code");
    assert_eq!(code.len(), 24);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(ferry_api::setup::code_path(app.dir.path())).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // The account itself is checked.
    let r = setup(&code, "not-an-email", &password).send(&app).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    let r = setup(&code, EMAIL, "short").send(&app).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.text().contains("at least 8 characters"), "{}", r.text());
    // A page of another site can't set the server up either.
    let r = setup(&code, EMAIL, &password).header("sec-fetch-site", "cross-site").send(&app).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::FORBIDDEN, "cross_site_request"));

    let r = setup(&code, " Ada@Example.com ", &password).send(&app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let body = r.json();
    assert_eq!((&body["setup_required"], &body["auth"]), (&json!(false), &json!("session")));
    assert_eq!(body["user"]["email"], EMAIL);
    assert!(body["user"]["id"].as_str().unwrap().starts_with("usr-"));
    let set_cookie = r.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("ferry_session=fys_"), "{set_cookie}");
    for attribute in ["HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(set_cookie.contains(attribute), "{attribute}: {set_cookie}");
    }
    assert!(!set_cookie.contains("Secure"), "plain HTTP: {set_cookie}");
    // The code served once.
    assert!(ferry_api::setup::read_code(app.dir.path()).is_none());
    assert_eq!(ferry_api::setup::ensure_code(&app.config, &app.store).await.unwrap(), None);

    // The browser that set the server up is signed in.
    let cookie = session_cookie(&r);
    let r = Call::get("/api/v1/auth/status").cookie(&cookie).send(&app).await;
    assert_eq!(r.json()["user"]["email"], EMAIL);
    assert_eq!(Call::get("/api/v1/info").cookie(&cookie).send(&app).await.status, StatusCode::OK);
    // Another one isn't, and can't set it up again.
    let r = Call::get("/api/v1/auth/status").send(&app).await;
    assert_eq!(r.json(), json!({"setup_required": false, "auth": null, "user": null}));
    let r = setup(&code, "eve@example.com", &password).send(&app).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    // Nothing of the password is stored as it is.
    let user = app.store.first_user().await.unwrap().unwrap();
    assert!(user.password_hash.starts_with("$argon2id$") && !user.password_hash.contains(&password));
}

#[tokio::test]
async fn signing_in_and_out() {
    let app = TestApp::new().await;
    let (password, _) = set_up(&app).await;

    let r = sign_in(&app, &new_password()).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::UNAUTHORIZED, "invalid_credentials"));
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    let wrong_password = r.text();
    // An unknown email is refused with the same words.
    let r =
        Call::post("/api/v1/auth/login", json!({"email": "eve@example.com", "password": password})).send(&app).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.text(), wrong_password);

    // The email is not case-sensitive.
    let r = Call::post("/api/v1/auth/login", json!({"email": "ADA@example.com", "password": password}))
        .header("x-forwarded-proto", "https")
        .send(&app)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["user"]["email"], EMAIL);
    assert!(r.headers[header::SET_COOKIE].to_str().unwrap().ends_with("; Secure"), "over HTTPS");
    let cookie = session_cookie(&r);

    // The cookie is what the API accepts from the dashboard.
    assert_eq!(Call::get("/api/v1/services").cookie(&cookie).send(&app).await.status, StatusCode::OK);
    let r = Call::get("/api/v1/services").send(&app).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("not signed in"), "{}", r.text());
    let made_up = format!("ferry_session=fys_{}", ferry_core::ids::random_secret(12));
    let r = Call::get("/api/v1/services").cookie(&made_up).send(&app).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("session has ended"), "{}", r.text());
    // Only the digest of the cookie is stored.
    let secret = cookie.strip_prefix("ferry_session=").unwrap();
    let stored = app.store.find_session(&ferry_core::auth::digest(secret)).await.unwrap().unwrap();
    assert_ne!(stored.token_hash, secret);

    let r = Call::new(Method::POST, "/api/v1/auth/logout").cookie(&cookie).send(&app).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(Call::get("/api/v1/services").cookie(&cookie).send(&app).await.status, StatusCode::UNAUTHORIZED);
    // Signing out twice, or without a session, is fine.
    assert_eq!(Call::new(Method::POST, "/api/v1/auth/logout").send(&app).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn servers_under_one_host_name_keep_a_session_each() {
    // Browsers keep cookies per host name, not per port: the cookie of the
    // server at :7878 is also sent to the one at :7879.
    let app = TestApp::new().await;
    let (password, _) = set_up(&app).await;
    let r = Call::post("/api/v1/auth/login", json!({"email": EMAIL, "password": password}))
        .header("host", "127.0.0.1:7878")
        .send(&app)
        .await;
    let cookie = session_cookie(&r);
    assert!(cookie.starts_with("ferry_session_7878=fys_"), "{cookie}");
    let info = |host: &'static str| Call::get("/api/v1/info").cookie(&cookie).header("host", host);
    assert_eq!(info("127.0.0.1:7878").send(&app).await.status, StatusCode::OK);
    // The other server doesn't take it for its own, and signing out there leaves it alone.
    assert_eq!(info("127.0.0.1:7879").send(&app).await.status, StatusCode::UNAUTHORIZED);
    let r = Call::new(Method::POST, "/api/v1/auth/logout")
        .cookie(&cookie)
        .header("host", "127.0.0.1:7879")
        .send(&app)
        .await;
    assert!(r.headers[header::SET_COOKIE].to_str().unwrap().starts_with("ferry_session_7879=;"));
    assert_eq!(info("127.0.0.1:7878").send(&app).await.status, StatusCode::OK);
    // Through the proxy, the port is the one the browser used.
    let r = Call::post("/api/v1/auth/login", json!({"email": EMAIL, "password": password}))
        .header("host", "127.0.0.1:7878")
        .header("x-forwarded-host", "ferry.localhost:8080")
        .send(&app)
        .await;
    assert!(session_cookie(&r).starts_with("ferry_session_8080=fys_"), "{}", session_cookie(&r));
}

#[tokio::test]
async fn a_session_ends_when_it_is_not_used() {
    let app = TestApp::new().await;
    let (_, cookie) = set_up(&app).await;
    let user = app.store.first_user().await.unwrap().unwrap();
    let session = app.store.list_sessions(&user.id).await.unwrap().remove(0);
    let now = chrono::Utc::now();

    // Used a while ago: the next request pushes its end back.
    let soon = now + chrono::Duration::hours(1);
    app.store.touch_session(&session.id, now - chrono::Duration::hours(2), soon).await.unwrap();
    assert_eq!(Call::get("/api/v1/info").cookie(&cookie).send(&app).await.status, StatusCode::OK);
    let renewed = app.store.list_sessions(&user.id).await.unwrap().remove(0);
    assert!(renewed.expires_at > now + chrono::Duration::days(29), "{}", renewed.expires_at);

    // Past its end: signed out.
    app.store.touch_session(&session.id, now, now - chrono::Duration::seconds(1)).await.unwrap();
    assert_eq!(Call::get("/api/v1/info").cookie(&cookie).send(&app).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(Call::get("/api/v1/auth/status").cookie(&cookie).send(&app).await.json()["auth"], Value::Null);
}

#[tokio::test]
async fn a_session_only_changes_things_from_the_dashboard() {
    let app = TestApp::new().await;
    let (_, cookie) = set_up(&app).await;
    let create = |name: &str| Call::post("/api/v1/env-groups", json!({"name": name})).cookie(&cookie);

    // Another site made the browser send the cookie.
    let r = create("a").header("sec-fetch-site", "cross-site").send(&app).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::FORBIDDEN, "cross_site_request"));
    // (Reached at an address with a port, the cookie is named after the port.)
    let at_7878 = cookie.replace("ferry_session=", "ferry_session_7878=");
    let from = |origin: &'static str, name: &'static str| {
        Call::post("/api/v1/env-groups", json!({"name": name}))
            .cookie(&at_7878)
            .header("origin", origin)
            .header("host", "ferry.test:7878")
    };
    let r = from("http://evil.test", "a").send(&app).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(app.store.list_env_groups().await.unwrap().is_empty());

    // The dashboard's own pages.
    let r = create("a").header("sec-fetch-site", "same-origin").send(&app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let r = from("http://ferry.test:7878", "b").send(&app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    // Reading changes nothing: no origin is asked.
    let r = Call::get("/api/v1/env-groups").cookie(&cookie).header("sec-fetch-site", "cross-site").send(&app).await;
    assert_eq!(r.status, StatusCode::OK);
    // An API token isn't sent by a browser on its own: no origin is asked either.
    let r = Request::post("/api/v1/env-groups")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("sec-fetch-site", "cross-site")
        .body(Body::from(r#"{"name":"c"}"#))
        .unwrap();
    assert_eq!(app.send(r).await.status, StatusCode::CREATED);
    // Signing out is the dashboard's too.
    let r = Call::new(Method::POST, "/api/v1/auth/logout")
        .cookie(&cookie)
        .header("sec-fetch-site", "cross-site")
        .send(&app)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(Call::get("/api/v1/info").cookie(&cookie).send(&app).await.status, StatusCode::OK);
}

#[tokio::test]
async fn failed_sign_ins_are_limited() {
    let app = TestApp::new().await;
    let (password, _) = set_up(&app).await;
    for _ in 0..10 {
        assert_eq!(sign_in(&app, &new_password()).await.status, StatusCode::UNAUTHORIZED);
    }
    // Now even the right password waits.
    let r = sign_in(&app, &password).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::TOO_MANY_REQUESTS, "too_many_attempts"));
    assert!(r.text().contains("try again in"), "{}", r.text());
}

#[tokio::test]
async fn changing_the_password_ends_the_other_sessions() {
    let app = TestApp::new().await;
    let (password, here) = set_up(&app).await;
    let elsewhere = session_cookie(&sign_in(&app, &password).await);
    let new = new_password();
    let change = |current: &str, new: &str| {
        Call::post("/api/v1/auth/password", json!({"current_password": current, "new_password": new}))
    };

    let r = change(&password, &new).send(&app).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "no session");
    let r = app.post("/api/v1/auth/password", json!({"current_password": password, "new_password": new})).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::FORBIDDEN, "session_required"));
    let r = change(&new_password(), &new).cookie(&here).send(&app).await;
    assert_eq!((r.status, r.code().as_str()), (StatusCode::UNAUTHORIZED, "invalid_credentials"));
    let r = change(&password, "short").cookie(&here).send(&app).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let r = change(&password, &new).cookie(&here).send(&app).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.text());
    assert_eq!(Call::get("/api/v1/info").cookie(&here).send(&app).await.status, StatusCode::OK);
    assert_eq!(Call::get("/api/v1/info").cookie(&elsewhere).send(&app).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(sign_in(&app, &password).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(sign_in(&app, &new).await.status, StatusCode::OK);
}

#[tokio::test]
async fn sessions_are_listed_and_ended() {
    let app = TestApp::new().await;
    let (password, here) = set_up(&app).await;
    let r = Call::post("/api/v1/auth/login", json!({"email": EMAIL, "password": password}))
        .header("user-agent", "Other Browser/1.0")
        .send(&app)
        .await;
    let elsewhere = session_cookie(&r);

    let r = Call::get("/api/v1/auth/sessions").cookie(&here).send(&app).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let sessions = r.json();
    let sessions = sessions.as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions.iter().filter(|s| s["current"] == true).count(), 1);
    let other = sessions.iter().find(|s| s["current"] == false).unwrap();
    assert_eq!(other["user_agent"], "Other Browser/1.0");
    assert!(other.get("token_hash").is_none() && !r.text().contains("fys_"), "{}", r.text());
    let other_id = other["id"].as_str().unwrap().to_string();
    assert_eq!(app.get("/api/v1/auth/sessions").await.code(), "session_required");

    let end = |id: &str| format!("/api/v1/auth/sessions/{id}");
    let r = Call::new(Method::DELETE, &end(&other_id)).cookie(&here).send(&app).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.headers.get(header::SET_COOKIE).is_none(), "this browser stays signed in");
    assert_eq!(Call::get("/api/v1/info").cookie(&elsewhere).send(&app).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(Call::new(Method::DELETE, &end(&other_id)).cookie(&here).send(&app).await.status, StatusCode::NOT_FOUND);

    // Ending one's own session is signing out.
    let mine = sessions.iter().find(|s| s["current"] == true).unwrap()["id"].as_str().unwrap().to_string();
    let r = Call::new(Method::DELETE, &end(&mine)).cookie(&here).send(&app).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(Call::get("/api/v1/info").cookie(&here).send(&app).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_tokens_are_shown_once_and_revoked() {
    let app = TestApp::new().await;
    let (_, cookie) = set_up(&app).await;
    let create = |body: Value| Call::post("/api/v1/auth/tokens", body).cookie(&cookie);

    for bad in [json!({"name": ""}), json!({"name": "ci", "expires_in_days": 0}), json!({"name": "x".repeat(101)})] {
        assert_eq!(create(bad.clone()).send(&app).await.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let r = create(json!({"name": " CI "})).send(&app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let created = r.json();
    let token = created["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("fy_") && token.len() == 43, "{token}");
    assert_eq!(created["api_token"]["name"], "CI");
    assert_eq!(created["api_token"]["hint"], token[39..]);
    assert_eq!(created["api_token"]["expires_at"], Value::Null);
    let id = created["api_token"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("tok-"));
    let r = create(json!({"name": "temp", "expires_in_days": 7})).send(&app).await;
    assert!(r.json()["api_token"]["expires_at"].is_string(), "{}", r.text());

    // The list never shows a token again, and the store has none.
    let r = Call::get("/api/v1/auth/tokens").cookie(&cookie).send(&app).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.text().contains(&token), "{}", r.text());
    let listed = r.json();
    assert_eq!(listed.as_array().unwrap().len(), 2);
    let ci = listed.as_array().unwrap().iter().find(|t| t["id"] == id.as_str()).unwrap();
    assert_eq!(ci["last_used_at"], Value::Null);
    let stored = app.store.list_api_tokens().await.unwrap();
    assert!(stored.iter().all(|t| t.token_hash != token && t.token_hash.len() == 64));

    // It authenticates like the server token does...
    let with_token = |method: Method, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(app.send(with_token(Method::GET, "/api/v1/services")).await.status, StatusCode::OK);
    let r = app.send(Request::get(format!("/api/v1/info?access_token={token}")).body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = Call::get("/api/v1/auth/status").header("authorization", &format!("Bearer {token}")).send(&app).await;
    assert_eq!(r.json()["auth"], "token");
    let r = Call::get("/api/v1/auth/tokens").cookie(&cookie).send(&app).await;
    let used = r.json().as_array().unwrap().iter().find(|t| t["id"] == id.as_str()).unwrap()["last_used_at"].clone();
    assert!(used.is_string(), "its use is recorded: {used}");
    // ...except on the account: a token makes no tokens.
    for (method, uri) in [
        (Method::GET, "/api/v1/auth/tokens".to_string()),
        (Method::POST, "/api/v1/auth/tokens".to_string()),
        (Method::DELETE, format!("/api/v1/auth/tokens/{id}")),
        (Method::GET, "/api/v1/auth/sessions".to_string()),
    ] {
        let r = app.send(with_token(method.clone(), &uri)).await;
        assert_eq!((r.status, r.code().as_str()), (StatusCode::FORBIDDEN, "session_required"), "{method} {uri}");
    }

    let revoke = format!("/api/v1/auth/tokens/{id}");
    assert_eq!(Call::new(Method::DELETE, &revoke).cookie(&cookie).send(&app).await.status, StatusCode::NO_CONTENT);
    let r = app.send(with_token(Method::GET, "/api/v1/services")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("invalid API token"), "{}", r.text());
    assert_eq!(Call::new(Method::DELETE, &revoke).cookie(&cookie).send(&app).await.status, StatusCode::NOT_FOUND);

    // A token past its end is refused too.
    let expired = ferry_core::auth::new_api_token();
    let yesterday = chrono::Utc::now() - chrono::Duration::days(1);
    app.store.create_api_token(&ferry_core::ApiToken::new("old", &expired, Some(yesterday))).await.unwrap();
    let r = app
        .send(
            Request::get("/api/v1/services")
                .header(header::AUTHORIZATION, format!("Bearer {expired}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // The server token still works, and isn't in the list.
    assert_eq!(app.get("/api/v1/services").await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_terminal_logs_in_through_the_dashboard() {
    let app = TestApp::new().await;
    let start = || Call::post("/api/v1/auth/cli", json!({"name": "ada@laptop"}));
    // Nobody can approve on a server without an account.
    assert_eq!(start().send(&app).await.status, StatusCode::CONFLICT);
    let (_, cookie) = set_up(&app).await;

    let r = start().send(&app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let started = r.json();
    let (id, code, secret) = (
        started["id"].as_str().unwrap().to_string(),
        started["code"].as_str().unwrap().to_string(),
        started["secret"].as_str().unwrap().to_string(),
    );
    assert_eq!((&started["expires_in"], &started["interval"]), (&json!(600), &json!(2)));
    let poll_uri = format!("/api/v1/auth/cli/{id}/token");
    let poll = |secret: &str| Call::post(&poll_uri, json!({"secret": secret}));
    assert_eq!(poll(&secret).send(&app).await.json(), json!({"status": "pending", "token": null}));
    assert_eq!(poll("another-terminal").send(&app).await.status, StatusCode::UNAUTHORIZED);

    // The approval page: the signed-in dashboard only.
    let page = format!("/api/v1/auth/cli/{id}");
    assert_eq!(Call::get(&page).send(&app).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.get(&page).await.code(), "session_required");
    let r = Call::get(&page).cookie(&cookie).send(&app).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let view = r.json();
    assert_eq!(
        (&view["name"], &view["code"], &view["status"]),
        (&json!("ada@laptop"), &json!(code), &json!("pending"))
    );
    assert!(view.get("secret").is_none(), "{view}");

    let approve = format!("{page}/approve");
    assert_eq!(app.post(&approve, json!({})).await.code(), "session_required");
    let r = Call::new(Method::POST, &approve).cookie(&cookie).send(&app).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["status"], "approved");
    assert!(!r.text().contains("fy_"), "the page never sees the token: {}", r.text());
    assert_eq!(Call::new(Method::POST, &approve).cookie(&cookie).send(&app).await.status, StatusCode::NOT_FOUND);

    // The terminal collects its token, once.
    let r = poll(&secret).send(&app).await;
    assert_eq!(r.json()["status"], "approved");
    let token = r.json()["token"].as_str().unwrap().to_string();
    assert_eq!(poll(&secret).send(&app).await.status, StatusCode::NOT_FOUND);
    let r = app
        .send(
            Request::get("/api/v1/info")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    // It is a named token like any other: listed, revocable.
    let tokens = Call::get("/api/v1/auth/tokens").cookie(&cookie).send(&app).await.json();
    assert_eq!(tokens.as_array().unwrap().len(), 1);
    assert_eq!(tokens[0]["name"], "ada@laptop");

    // Denied: the terminal is told, and no token is made.
    let started = start().send(&app).await.json();
    let (id, secret) = (started["id"].as_str().unwrap(), started["secret"].as_str().unwrap());
    let r = Call::new(Method::POST, &format!("/api/v1/auth/cli/{id}/deny")).cookie(&cookie).send(&app).await;
    assert_eq!(r.json()["status"], "denied");
    let poll_uri = format!("/api/v1/auth/cli/{id}/token");
    let r = Call::post(&poll_uri, json!({"secret": secret})).send(&app).await;
    assert_eq!(r.json(), json!({"status": "denied", "token": null}));
    assert_eq!(Call::post(&poll_uri, json!({"secret": secret})).send(&app).await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.store.list_api_tokens().await.unwrap().len(), 1);

    // A terminal that says nothing about itself gets a default name.
    let r = Call::new(Method::POST, "/api/v1/auth/cli").send(&app).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let page = format!("/api/v1/auth/cli/{}", r.json()["id"].as_str().unwrap());
    assert_eq!(Call::get(&page).cookie(&cookie).send(&app).await.json()["name"], "ferry CLI");
    assert_eq!(Call::get("/api/v1/auth/cli/unknown").cookie(&cookie).send(&app).await.status, StatusCode::NOT_FOUND);
}
