//! GitHub Apps: how a Ferry server gets to read repositories on GitHub
//! without anybody pasting a token.
//!
//! 1. The server registers an app for itself from a **manifest**: the
//!    browser posts it to GitHub, the user confirms, and GitHub hands back a
//!    code that is exchanged for the app's id, secrets and private key.
//! 2. The user **installs** the app on the account, choosing the
//!    repositories it may read.
//! 3. The app's private key signs short-lived JWTs that mint installation
//!    tokens (see [`crate::access`]).
//!
//! No application has to exist beforehand, so this works for any server,
//! whatever its address.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ferry_core::{GithubApp, ids};
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde_json::json;

use crate::provider::ProviderError;

/// GitHub's limit for app names.
const MAX_NAME: usize = 34;

/// A name for the app of the server that is reached at `redirect_uri`:
/// `ferry-<host>-<random>`. App names are unique across GitHub, hence the
/// random end (the user can still change it on GitHub's page).
fn app_name(redirect_uri: &str) -> String {
    let host = ferry_core::git::parse_http_url(redirect_uri).map(|u| u.host).unwrap_or_default();
    let mut slug = String::new();
    for c in host.chars() {
        match c {
            c if c.is_ascii_alphanumeric() => slug.push(c.to_ascii_lowercase()),
            _ if !slug.ends_with('-') && !slug.is_empty() => slug.push('-'),
            _ => {}
        }
    }
    let suffix = ids::random_secret(6);
    // "ferry-" + host + "-" + suffix
    let room = MAX_NAME - "ferry-".len() - 1 - suffix.len();
    let slug: String = slug.chars().take(room).collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() { format!("ferry-{suffix}") } else { format!("ferry-{slug}-{suffix}") }
}

/// The manifest of the app: private to its owner, read access to code and
/// metadata, and every redirect back to `redirect_uri` (the dashboard's
/// `/git/callback` page), after the registration (`?code=`), after an
/// installation and after its repositories were changed (`?installation_id=`).
pub fn manifest(redirect_uri: &str) -> serde_json::Value {
    let home = ferry_core::git::parse_http_url(redirect_uri).map(|u| u.origin()).unwrap_or_else(|| redirect_uri.into());
    json!({
        "name": app_name(redirect_uri),
        "url": home,
        "description": format!("Lets the Ferry server at {home} read the repositories it deploys."),
        "redirect_url": redirect_uri,
        "callback_urls": [redirect_uri],
        "setup_url": redirect_uri,
        "setup_on_update": true,
        "public": false,
        "request_oauth_on_install": false,
        "default_permissions": { "contents": "read", "metadata": "read" },
    })
}

/// GitHub's page that registers an app from a manifest (posted to it as the
/// form field `manifest`): on the user's own account, or in `organization`.
pub fn registration_url(base_url: &str, organization: Option<&str>, state: &str) -> String {
    match organization {
        Some(org) => format!("{base_url}/organizations/{org}/settings/apps/new?state={state}"),
        None => format!("{base_url}/settings/apps/new?state={state}"),
    }
}

/// GitHub's page that installs the app on an account.
pub fn install_url(app: &GithubApp, state: &str) -> String {
    format!("{}/installations/new?state={state}", app.url.trim_end_matches('/'))
}

/// The private key of a PEM file: PKCS#1 (`RSA PRIVATE KEY`, what GitHub
/// hands out) or PKCS#8 (`PRIVATE KEY`).
fn key_pair(pem: &str) -> Result<RsaKeyPair, ProviderError> {
    let unreadable = || ProviderError::Internal("the GitHub App's private key can't be read".into());
    let mut label: Option<&str> = None;
    let mut body = String::new();
    for line in pem.lines().map(str::trim) {
        if let Some(l) = line.strip_prefix("-----BEGIN ").and_then(|l| l.strip_suffix("-----")) {
            label = Some(l);
            body.clear();
        } else if line.starts_with("-----END ") {
            break;
        } else if label.is_some() {
            body.push_str(line);
        }
    }
    let der = STANDARD.decode(body).map_err(|_| unreadable())?;
    match label {
        Some("RSA PRIVATE KEY") => RsaKeyPair::from_der(&der),
        Some("PRIVATE KEY") => RsaKeyPair::from_pkcs8(&der),
        _ => return Err(unreadable()),
    }
    .map_err(|_| unreadable())
}

/// A JWT that authenticates as the app for the next few minutes (RS256,
/// issued a minute ago to allow for clock drift).
pub fn jwt(app: &GithubApp, now: DateTime<Utc>) -> Result<String, ProviderError> {
    let key = key_pair(&app.private_key)?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = json!({ "iat": now.timestamp() - 60, "exp": now.timestamp() + 9 * 60, "iss": app.id.to_string() });
    let message = format!("{header}.{}", URL_SAFE_NO_PAD.encode(claims.to_string()));
    let mut signature = vec![0u8; key.public().modulus_len()];
    key.sign(&RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), message.as_bytes(), &mut signature)
        .map_err(|_| ProviderError::Internal("signing with the GitHub App's private key failed".into()))?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh RSA key in PEM, made with the `openssl` CLI (no key is kept
    /// in the repository). `None` when there is no `openssl`.
    pub(crate) fn generate_key() -> Option<String> {
        let out = std::process::Command::new("openssl").args(["genrsa", "2048"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `pem` rewritten by `openssl <args>` (read from a file).
    fn convert(pem: &str, args: &[&str]) -> Option<String> {
        let dir = tempfile::tempdir().ok()?;
        let path = dir.path().join("key.pem");
        std::fs::write(&path, pem).ok()?;
        let out = std::process::Command::new("openssl").args(args).arg("-in").arg(&path).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn app(private_key: String) -> GithubApp {
        GithubApp {
            id: 4242,
            slug: "ferry-test".into(),
            url: "https://github.com/apps/ferry-test".into(),
            private_key,
            webhook_secret: None,
        }
    }

    #[test]
    fn jwts_are_signed_with_the_apps_key() {
        let Some(key) = generate_key() else {
            eprintln!("skipped: openssl is not installed");
            return;
        };
        // Both encodings of the key: PKCS#8, and PKCS#1 (OpenSSL 3 needs
        // `-traditional` for it, LibreSSL writes nothing else).
        let pkcs8 = convert(&key, &["pkcs8", "-topk8", "-nocrypt"]).expect("openssl pkcs8");
        let pkcs1 = convert(&key, &["rsa", "-traditional"]).or_else(|| convert(&key, &["rsa"])).expect("openssl rsa");
        assert!(pkcs8.contains("BEGIN PRIVATE KEY") && pkcs1.contains("BEGIN RSA PRIVATE KEY"), "{pkcs8}{pkcs1}");
        let now = DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().with_timezone(&Utc);
        for pem in [pkcs1, pkcs8] {
            let token = jwt(&app(pem.clone()), now).unwrap();
            let parts: Vec<&str> = token.split('.').collect();
            assert_eq!(parts.len(), 3, "{token}");
            let decode = |s: &str| URL_SAFE_NO_PAD.decode(s).unwrap();
            let header: serde_json::Value = serde_json::from_slice(&decode(parts[0])).unwrap();
            assert_eq!(header, json!({"alg": "RS256", "typ": "JWT"}));
            let claims: serde_json::Value = serde_json::from_slice(&decode(parts[1])).unwrap();
            assert_eq!(claims, json!({"iat": now.timestamp() - 60, "exp": now.timestamp() + 540, "iss": "4242"}));
            // The signature verifies with the key's public half.
            let public = key_pair(&pem).unwrap();
            let verifier = ring::signature::UnparsedPublicKey::new(
                &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                public.public().as_ref().to_vec(),
            );
            let signed = format!("{}.{}", parts[0], parts[1]);
            verifier.verify(signed.as_bytes(), &decode(parts[2])).expect("a valid signature");
            assert!(verifier.verify(b"something else", &decode(parts[2])).is_err());
        }
    }

    #[test]
    fn unreadable_keys_are_reported_without_their_content() {
        // The right markers around something that is no key (put together
        // here: nothing in the repository looks like a key file).
        let label = "RSA PRIVATE KEY";
        let empty = format!("-----BEGIN {label}-----\nAAAA\n-----END {label}-----\n");
        for bad in ["", "not a key", empty.as_str()] {
            let err = jwt(&app(bad.to_string()), Utc::now()).unwrap_err();
            assert_eq!(err, ProviderError::Internal("the GitHub App's private key can't be read".into()), "{bad}");
        }
    }

    #[test]
    fn manifests_send_every_redirect_to_the_dashboard() {
        let m = manifest("https://ferry.example.com/git/callback");
        assert_eq!(m["url"], "https://ferry.example.com");
        for key in ["redirect_url", "setup_url"] {
            assert_eq!(m[key], "https://ferry.example.com/git/callback", "{key}");
        }
        assert_eq!(m["callback_urls"], json!(["https://ferry.example.com/git/callback"]));
        assert_eq!(m["public"], false);
        assert_eq!(m["setup_on_update"], true);
        assert_eq!(m["default_permissions"], json!({"contents": "read", "metadata": "read"}));
        // No webhook: nothing GitHub would have to reach.
        assert!(m.get("hook_attributes").is_none() && m.get("default_events").is_none());
        let name = m["name"].as_str().unwrap();
        assert!(name.starts_with("ferry-ferry-example-com-") && name.len() <= MAX_NAME, "{name}");
    }

    #[test]
    fn app_names_fit_githubs_limit() {
        let name = app_name("http://localhost:7878/git/callback");
        assert!(name.starts_with("ferry-localhost-") && name.len() == "ferry-localhost-".len() + 6, "{name}");
        // Unique from one registration to the next.
        assert_ne!(name, app_name("http://localhost:7878/git/callback"));
        let long = app_name("https://a-very-long-host-name.deploys.example.internal/git/callback");
        assert!(long.len() <= MAX_NAME && long.starts_with("ferry-a-very-long-host-name-"), "{long}");
        assert!(long.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{long}");
        let ip = app_name("http://[::1]:7878/git/callback");
        assert!(ip.starts_with("ferry-1-") || ip.starts_with("ferry-"), "{ip}");
        assert!(!app_name("not a url").contains("--"));
    }

    #[test]
    fn github_pages() {
        assert_eq!(
            registration_url("https://github.com", None, "abc"),
            "https://github.com/settings/apps/new?state=abc"
        );
        assert_eq!(
            registration_url("https://ghe.example.com", Some("acme"), "abc"),
            "https://ghe.example.com/organizations/acme/settings/apps/new?state=abc"
        );
        assert_eq!(
            install_url(&app(String::new()), "abc"),
            "https://github.com/apps/ferry-test/installations/new?state=abc"
        );
    }
}
