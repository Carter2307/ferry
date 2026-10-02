//! Input validation shared by the API, blueprints and the engine.

use crate::models::{Runtime, Service, ServiceType};
use crate::schedule::Schedule;
use crate::{Error, Result};

/// Names that cannot be used for services/datastores.
pub const RESERVED_NAMES: &[&str] = &["ferry", "localhost"];

/// Maximum instances per service.
pub const MAX_INSTANCES: u32 = 50;

/// Maximum size of one environment variable value (bytes).
pub const MAX_ENV_VALUE_BYTES: usize = 32 * 1024;

/// Maximum total size of a service's (or env group's) variables, keys + values.
/// Keeps process environments and `docker build` well under OS `ARG_MAX`.
pub const MAX_ENV_TOTAL_BYTES: usize = 256 * 1024;

/// True if `s` has the shape of a Ferry id (`srv-`, `dep-`, `job-`, `dbs-`,
/// `evg-` or `git-` followed by 20 hex chars). Such names are rejected so
/// id-or-name lookups can never be ambiguous.
pub fn looks_like_id(s: &str) -> bool {
    use crate::ids;
    [ids::SERVICE, ids::DEPLOY, ids::JOB, ids::DATASTORE, ids::ENV_GROUP, ids::GIT_CONNECTION].iter().any(|p| {
        ids::has_prefix(s, p) && s[p.len() + 1..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Service / datastore names: DNS label, 1–40 chars, `[a-z0-9-]`, starts with a
/// letter, doesn't end with `-`. They double as private-network hostnames.
pub fn resource_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name.as_bytes()[0].is_ascii_lowercase()
        && !name.ends_with('-')
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        return Err(Error::invalid(format!(
            "invalid name '{name}': use 1-40 lowercase letters, digits or '-', starting with a letter"
        )));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(Error::invalid(format!("name '{name}' is reserved")));
    }
    if looks_like_id(name) {
        return Err(Error::invalid(format!("name '{name}' looks like a resource id; choose another name")));
    }
    Ok(())
}

/// Env group names: 1–64 chars of `[A-Za-z0-9_.-]`.
pub fn env_group_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !ok {
        return Err(Error::invalid(format!(
            "invalid env group name '{name}': use 1-64 letters, digits, '-', '_' or '.'"
        )));
    }
    if looks_like_id(name) {
        return Err(Error::invalid(format!("env group name '{name}' looks like a resource id; choose another name")));
    }
    Ok(())
}

/// Environment variable keys: non-empty, no `=`, whitespace or NUL, ≤ 256 chars.
pub fn env_key(key: &str) -> Result<()> {
    let ok = !key.is_empty() && key.len() <= 256 && !key.chars().any(|c| c == '=' || c == '\0' || c.is_whitespace());
    if ok { Ok(()) } else { Err(Error::invalid(format!("invalid environment variable name '{key}'"))) }
}

/// Validate one variable (key rules plus value size / no NUL bytes).
pub fn env_var(key: &str, value: &str) -> Result<()> {
    env_key(key)?;
    if value.len() > MAX_ENV_VALUE_BYTES {
        return Err(Error::invalid(format!(
            "value of {key} is {} bytes; the maximum is {MAX_ENV_VALUE_BYTES} bytes",
            value.len()
        )));
    }
    if value.contains('\0') {
        return Err(Error::invalid(format!("value of {key} contains a NUL byte")));
    }
    Ok(())
}

/// Validate a whole set of variables: each variable, no duplicate keys, and
/// the total size limit.
pub fn env_vars(vars: &[crate::models::EnvVar]) -> Result<()> {
    let mut total = 0usize;
    let mut seen = std::collections::HashSet::new();
    for v in vars {
        env_var(&v.key, &v.value)?;
        if !seen.insert(v.key.as_str()) {
            return Err(Error::invalid(format!("environment variable {} is set more than once", v.key)));
        }
        total += v.key.len() + v.value.len() + 2;
    }
    if total > MAX_ENV_TOTAL_BYTES {
        return Err(Error::invalid(format!(
            "environment variables total {total} bytes; the maximum is {MAX_ENV_TOTAL_BYTES} bytes"
        )));
    }
    Ok(())
}

/// A git commit reference given by a user: 4–64 hex characters.
pub fn commit(sha: &str) -> Result<()> {
    let sha = sha.trim();
    if (4..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid commit '{sha}': expected a hex commit sha")))
    }
}

/// A git branch name (subset of `git check-ref-format --branch` rules).
pub fn branch(b: &str) -> Result<()> {
    let bad = b.is_empty()
        || b.len() > 255
        || b.starts_with('-')
        || b.starts_with('/')
        || b.ends_with('/')
        || b.ends_with('.')
        || b.ends_with(".lock")
        || b.contains("..")
        || b.contains("//")
        || b.contains("@{")
        || b.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if bad { Err(Error::invalid(format!("invalid branch '{b}'"))) } else { Ok(()) }
}

/// A repository URL: `https://…`, `http://…`, `ssh://…`, `git://…`,
/// `file:///abs/path`, scp-style `user@host:path`, or an absolute local path.
/// Relative paths are rejected (they would resolve against the server's
/// working directory, not the user's).
pub fn repo_url(url: &str) -> Result<()> {
    let u = url.trim();
    let invalid = |why: &str| Err(Error::invalid(format!("invalid repo_url '{u}': {why}")));
    if u.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return invalid("must not be empty or contain whitespace");
    }
    if u.starts_with('-') {
        return invalid("must not start with '-'");
    }
    if u.contains("::") {
        return invalid("git remote-helper transports (ext::, fd::, …) are not supported");
    }
    if u.contains('\\') {
        return invalid("must not contain backslashes");
    }
    let has_parent = |path: &str| path.split('/').any(|seg| seg == "..");
    if let Some(idx) = u.find("://") {
        let scheme = &u[..idx];
        let rest = &u[idx + 3..];
        return match scheme {
            "file" if !rest.starts_with('/') => invalid("file:// URLs must use an absolute path (file:///path)"),
            "file" if has_parent(rest) => invalid("'..' is not allowed in local paths"),
            "https" | "http" | "ssh" | "git" | "file" if !rest.is_empty() => Ok(()),
            _ => invalid("unsupported scheme (use https://, ssh://, git://, file:// or an absolute path)"),
        };
    }
    if u.starts_with("file:") {
        return invalid("file URLs must look like file:///absolute/path");
    }
    if u.starts_with('/') {
        return if has_parent(u) { invalid("'..' is not allowed in local paths") } else { Ok(()) };
    }
    // scp-like: [user@]host:path (host without '/', at least 2 chars so a
    // Windows drive letter like C: isn't mistaken for a host).
    if let Some(colon) = u.find(':') {
        let host = &u[..colon];
        let host_name = host.rsplit('@').next().unwrap_or(host);
        if host_name.len() > 1 && !host.contains('/') && colon + 1 < u.len() {
            return Ok(());
        }
    }
    invalid("local repositories must be given as an absolute path (or file:// URL)")
}

/// Validate and normalize the web URL of a git provider instance, e.g.
/// `https://gitlab.example.com` (a path below the host is kept): lowercase
/// host, default port dropped, no trailing slash. Credentials, query strings
/// and fragments are refused.
pub fn git_base_url(url: &str) -> Result<String> {
    let u = url.trim();
    let invalid = |why: &str| Err(Error::invalid(format!("invalid base_url '{}': {why}", crate::git::redact_url(u))));
    if u.contains(['?', '#']) {
        return invalid("it must not have a query string or a fragment");
    }
    let Some(parsed) = crate::git::parse_http_url(u) else {
        return invalid("expected an http(s) URL without credentials, such as https://gitlab.example.com");
    };
    let path = parsed.path.trim_end_matches('/');
    let plain = path.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b'~'));
    if !plain || path.split('/').skip(1).any(|seg| seg.is_empty() || seg == "." || seg == "..") {
        return invalid("its path may only use letters, digits, '.', '_', '-' and '~'");
    }
    Ok(format!("{}{path}", parsed.origin()))
}

/// An access token of a git provider: printable ASCII without spaces, at
/// most 1024 characters. Returns it trimmed.
pub fn git_token(token: &str) -> Result<&str> {
    let t = token.trim();
    if t.is_empty() {
        return Err(Error::invalid("the access token is empty"));
    }
    if t.len() > 1024 || !t.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::invalid(
            "invalid access token: expected at most 1024 printable characters without spaces (check that it was copied completely)",
        ));
    }
    Ok(t)
}

/// Where a git provider sends the browser back after an authorization: an
/// http(s) URL without credentials, query string or fragment (the provider
/// appends its own parameters). Returns it trimmed.
pub fn git_redirect_uri(url: &str) -> Result<&str> {
    let u = url.trim();
    let plain = u.len() <= 2048
        && !u.contains(['?', '#'])
        && crate::git::parse_http_url(u).is_some_and(|p| p.path.bytes().all(|b| b.is_ascii_graphic()));
    if plain {
        Ok(u)
    } else {
        Err(Error::invalid(
            "invalid redirect_uri: expected the http(s) URL of the dashboard's /git/callback page, without credentials, query string or fragment",
        ))
    }
}

/// The login of a GitHub organization: 1 to 39 letters, digits or `-`, not
/// at its ends. Returns it trimmed.
pub fn git_organization(name: &str) -> Result<&str> {
    let n = name.trim();
    let ok = (1..=39).contains(&n.len())
        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !n.starts_with('-')
        && !n.ends_with('-');
    if ok {
        Ok(n)
    } else {
        Err(Error::invalid(format!("invalid organization '{n}': expected its GitHub login (letters, digits and '-')")))
    }
}

/// The id or the secret of an OAuth application (`what` names it in the
/// error): printable ASCII without spaces, at most 256 characters. Returns
/// it trimmed.
pub fn git_client_credential<'a>(what: &str, value: &'a str) -> Result<&'a str> {
    let v = value.trim();
    if v.is_empty() {
        return Err(Error::invalid(format!("the {what} is empty")));
    }
    if v.len() > 256 || !v.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::invalid(format!(
            "invalid {what}: expected at most 256 printable characters without spaces (check that it was copied completely)"
        )));
    }
    Ok(v)
}

/// Validate and normalize (lowercase, no trailing dot) a custom domain.
pub fn domain(d: &str) -> Result<String> {
    let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let ok = d.len() <= 253 && d.contains('.') && d.split('.').all(label_ok);
    if !ok {
        return Err(Error::invalid(format!("invalid domain '{d}'")));
    }
    let tld = d.rsplit('.').next().unwrap_or_default();
    if tld.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::invalid(format!("invalid domain '{d}': IP addresses can't be used as custom domains")));
    }
    Ok(d)
}

/// Absolute mount path other than `/`.
pub fn mount_path(p: &str) -> Result<()> {
    if p.starts_with('/') && p.len() > 1 && !p.contains("..") {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid disk mount path '{p}': must be an absolute path other than '/'")))
    }
}

/// HTTP path starting with `/`.
pub fn health_check_path(p: &str) -> Result<()> {
    if p.starts_with('/') && !p.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid health check path '{p}': must start with '/'")))
    }
}

/// Check a whole service for internal consistency. Call after applying user
/// input and [`normalize_service`].
pub fn service(svc: &Service) -> Result<()> {
    resource_name(&svc.name)?;
    if svc.repo_url.is_some() && svc.image.is_some() {
        return Err(Error::invalid("set either repo_url or image, not both"));
    }
    if svc.runtime == Runtime::Image && svc.image.is_none() {
        return Err(Error::invalid("runtime 'image' requires an image"));
    }
    if let Some(r) = &svc.repo_url {
        repo_url(r)?;
    }
    if let Some(i) = &svc.image
        && (i.trim().is_empty() || i.chars().any(char::is_whitespace))
    {
        return Err(Error::invalid(format!("invalid image '{i}'")));
    }
    branch(&svc.branch)?;
    // dockerfile_path may use `../` (relative to root_dir) as long as the
    // builder keeps it inside the checkout; the others must stay below it.
    for (what, p, allow_parent) in [
        ("root_dir", &svc.root_dir, false),
        ("dockerfile_path", &svc.dockerfile_path, true),
        ("publish_dir", &svc.publish_dir, false),
    ] {
        if let Some(p) = p
            && (p.starts_with('/')
                || p.starts_with('-')
                || (!allow_parent && p.split(['/', '\\']).any(|seg| seg == "..")))
        {
            return Err(Error::invalid(format!("invalid {what} '{p}': must be a relative path inside the repository")));
        }
    }
    match (svc.service_type, &svc.schedule) {
        (ServiceType::CronJob, Some(s)) => {
            Schedule::parse(s)?;
        }
        (ServiceType::CronJob, None) => return Err(Error::invalid("cron jobs require a schedule")),
        (_, Some(_)) => return Err(Error::invalid("only cron jobs can have a schedule")),
        _ => {}
    }
    if svc.instances == 0 || svc.instances > MAX_INSTANCES {
        return Err(Error::invalid(format!(
            "instances must be between 1 and {MAX_INSTANCES} (use suspend to stop a service)"
        )));
    }
    if let Some(p) = &svc.disk_mount_path {
        mount_path(p)?;
        if svc.instances > 1 {
            return Err(Error::invalid("services with a disk are limited to 1 instance"));
        }
        if svc.service_type == ServiceType::StaticSite || svc.service_type == ServiceType::CronJob {
            return Err(Error::invalid("disks are only supported on web services, private services and workers"));
        }
    }
    if !svc.custom_domains.is_empty() && !svc.is_public_http() {
        return Err(Error::invalid("custom domains are only supported on web services and static sites"));
    }
    for d in &svc.custom_domains {
        domain(d)?;
    }
    if let Some(p) = &svc.health_check_path {
        if !svc.listens() {
            return Err(Error::invalid("health checks are only supported on services that listen on a port"));
        }
        health_check_path(p)?;
    }
    if svc.port == Some(0) {
        return Err(Error::invalid("port must be between 1 and 65535"));
    }
    crate::resources::validate(svc.memory_limit_mb, svc.cpu_limit)?;
    Ok(())
}

/// Normalize user input in place: trims strings, turns empty strings into
/// `None`, lowercases/dedups domains, and forces runtime `image` when an image
/// is set.
pub fn normalize_service(svc: &mut Service) {
    fn clean(o: &mut Option<String>) {
        if let Some(s) = o {
            let t = s.trim();
            if t.is_empty() {
                *o = None;
            } else if t.len() != s.len() {
                *o = Some(t.to_string());
            }
        }
    }
    for f in [
        &mut svc.repo_url,
        &mut svc.image,
        &mut svc.root_dir,
        &mut svc.dockerfile_path,
        &mut svc.build_command,
        &mut svc.start_command,
        &mut svc.publish_dir,
        &mut svc.health_check_path,
        &mut svc.schedule,
        &mut svc.disk_mount_path,
    ] {
        clean(f);
    }
    svc.name = svc.name.trim().to_string();
    svc.branch = svc.branch.trim().to_string();
    if svc.branch.is_empty() {
        svc.branch = "main".to_string();
    }
    if svc.image.is_some() {
        svc.runtime = Runtime::Image;
    } else if svc.runtime == Runtime::Image {
        // runtime image without an image is caught by `service()`.
    }
    let mut domains: Vec<String> = Vec::new();
    for d in &svc.custom_domains {
        let d = d.trim().trim_end_matches('.').to_ascii_lowercase();
        if !d.is_empty() && !domains.contains(&d) {
            domains.push(d);
        }
    }
    svc.custom_domains = domains;
    svc.cpu_limit = svc.cpu_limit.map(crate::resources::round_cpus);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(resource_name("web").is_ok());
        assert!(resource_name("my-api-2").is_ok());
        assert!(resource_name("2web").is_err());
        assert!(resource_name("Web").is_err());
        assert!(resource_name("web-").is_err());
        assert!(resource_name("ferry").is_err());
        assert!(resource_name(&"a".repeat(41)).is_err());
    }

    #[test]
    fn ids_repos_branches_env() {
        assert!(resource_name("srv-0123456789abcdef0123").is_err());
        assert!(resource_name("srv-web").is_ok());
        assert!(env_group_name("evg-0123456789abcdef0123").is_err());
        assert!(repo_url("https://github.com/a/b").is_ok());
        assert!(repo_url("git@github.com:a/b.git").is_ok());
        assert!(repo_url("/abs/path").is_ok());
        assert!(repo_url("file:///abs/path").is_ok());
        assert!(repo_url("./rel").is_err());
        assert!(repo_url("rel/path").is_err());
        assert!(repo_url("--upload-pack=x").is_err());
        assert!(repo_url("ftp://x/y").is_err());
        for bad in [
            "ext::sh -c touch%S",
            "fd::3",
            "file://./x",
            "file://x",
            "file:x",
            "file:///../x",
            "/a/../b",
            "C:\\x",
            "C:x",
        ] {
            assert!(repo_url(bad).is_err(), "{bad}");
        }
        assert!(repo_url("ssh://git@github.com:22/a/b").is_ok());
        assert!(branch("feature/x-1").is_ok());
        assert!(branch("-evil").is_err());
        assert!(branch("a..b").is_err());
        assert!(commit("abc123").is_ok());
        assert!(commit("--help").is_err());
        assert!(env_var("K", &"x".repeat(MAX_ENV_VALUE_BYTES + 1)).is_err());
        let dup = vec![crate::models::EnvVar::new("A", "1"), crate::models::EnvVar::new("A", "2")];
        assert!(env_vars(&dup).is_err());
        assert!(domain("127.0.0.1").is_err());
        assert!(domain("app.example.com").is_ok());
    }

    #[test]
    fn git_connection_inputs() {
        assert_eq!(git_base_url(" https://GitLab.Example.com/ ").unwrap(), "https://gitlab.example.com");
        assert_eq!(git_base_url("https://example.com:443/gitlab/").unwrap(), "https://example.com/gitlab");
        assert_eq!(git_base_url("http://127.0.0.1:8929").unwrap(), "http://127.0.0.1:8929");
        for bad in [
            "",
            "gitlab.example.com",
            "ssh://git@gitlab.example.com",
            "https://user:glpat-secret@gitlab.example.com",
            "https://gitlab.example.com/?next=x",
            "https://gitlab.example.com/#x",
            "https://gitlab.example.com/a/../b",
            "https://gitlab.example.com/a//b",
            "https://gitlab.example.com/a b",
        ] {
            let err = git_base_url(bad).unwrap_err().to_string();
            assert!(err.starts_with("invalid base_url") && !err.contains("glpat-secret"), "{bad}: {err}");
        }

        assert_eq!(git_token("  ghp_abcDEF123  ").unwrap(), "ghp_abcDEF123");
        assert!(git_token("   ").is_err());
        assert!(git_token("ghp_abc def").is_err());
        assert!(git_token("ghp_abc\ndef").is_err());
        assert!(git_token(&"a".repeat(1025)).is_err());
        // The message never repeats the token.
        assert!(!git_token("tok\u{e9}n-s3cret").unwrap_err().to_string().contains("s3cret"));

        // What a browser authorization is given.
        assert_eq!(
            git_redirect_uri(" http://localhost:7878/git/callback ").unwrap(),
            "http://localhost:7878/git/callback"
        );
        assert!(git_redirect_uri("https://ferry.example.com/git/callback").is_ok());
        for bad in [
            "",
            "/git/callback",
            "ferry.example.com/git/callback",
            "javascript:alert(1)",
            "https://user:pw@ferry.example.com/git/callback",
            "https://ferry.example.com/git/callback?next=x",
            "https://ferry.example.com/git/callback#x",
            "https://ferry.example.com/git call",
        ] {
            assert!(git_redirect_uri(bad).is_err(), "{bad}");
        }
        assert_eq!(git_organization(" acme-corp ").unwrap(), "acme-corp");
        for bad in ["", "-acme", "acme-", "acme corp", "acme/corp", "a?b", &"a".repeat(40)] {
            assert!(git_organization(bad).is_err(), "{bad}");
        }
        assert_eq!(git_client_credential("application id", " 0a1b2c ").unwrap(), "0a1b2c");
        let err = git_client_credential("application secret", "gloas-ab cd").unwrap_err().to_string();
        assert!(err.starts_with("invalid application secret") && !err.contains("gloas"), "{err}");
        assert!(git_client_credential("application id", "  ").unwrap_err().to_string().contains("is empty"));
    }

    #[test]
    fn domains_and_keys() {
        assert_eq!(domain("App.Example.COM.").unwrap(), "app.example.com");
        assert!(domain("localhost").is_err());
        assert!(domain("-a.com").is_err());
        assert!(env_key("DATABASE_URL").is_ok());
        assert!(env_key("A=B").is_err());
        assert!(env_key("").is_err());
    }

    #[test]
    fn service_rules() {
        let mut s = Service::new("job", ServiceType::CronJob);
        assert!(service(&s).is_err());
        s.schedule = Some("*/5 * * * *".into());
        assert!(service(&s).is_ok());
        let mut w = Service::new("web", ServiceType::WebService);
        w.image = Some(" nginx:alpine ".into());
        w.custom_domains = vec!["A.com".into(), "a.com".into()];
        normalize_service(&mut w);
        assert_eq!(w.runtime, Runtime::Image);
        assert_eq!(w.image.as_deref(), Some("nginx:alpine"));
        assert_eq!(w.custom_domains, vec!["a.com"]);
        assert!(service(&w).is_ok());
        w.disk_mount_path = Some("/data".into());
        w.instances = 2;
        assert!(service(&w).is_err());
        let mut worker = Service::new("bg", ServiceType::BackgroundWorker);
        worker.custom_domains = vec!["x.com".into()];
        assert!(service(&worker).is_err());
    }
}
