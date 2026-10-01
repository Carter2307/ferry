//! Validation rules shared by the REST handlers and blueprints.

use ferry_core::{Config, EnvVar, Error, Result, Service, Store, env, git, validate};

/// The git connection a service names must exist and be for its repository:
/// a token is only ever used on its own provider instance, for an http(s)
/// URL that carries no credentials of its own.
pub async fn git_connection(store: &Store, svc: &Service) -> Result<()> {
    let Some(id) = &svc.git_connection_id else { return Ok(()) };
    let Some(connection) = store.get_git_connection(id).await? else {
        return Err(Error::invalid(format!("git connection '{id}' not found")));
    };
    let repo = svc.repo_url.as_deref().unwrap_or_default();
    if !connection.serves(repo) {
        return Err(Error::invalid(format!(
            "the {} can't be used for {}: its token is only for http(s) repositories on {} (and URLs without credentials of their own)",
            connection.describe(),
            git::redact_url(repo),
            connection.base_url
        )));
    }
    Ok(())
}

/// Size of a set of variables as counted by [`validate::env_vars`].
fn env_bytes(vars: &[EnvVar]) -> usize {
    vars.iter().map(|v| v.key.len() + v.value.len() + 2).sum()
}

/// The combined environment of the service `name` must stay within
/// [`validate::MAX_ENV_TOTAL_BYTES`]: it becomes the build args of every
/// build and the container environment. `layers` are the variables of its
/// linked env groups (in link order) followed by its own.
pub fn effective_env_size(name: &str, layers: &[Vec<EnvVar>]) -> Result<()> {
    let total = env_bytes(&env::merge(layers));
    if total > validate::MAX_ENV_TOTAL_BYTES {
        return Err(Error::invalid(format!(
            "the environment of service '{name}' (its own variables plus linked env groups) would total {total} bytes; the maximum is {} bytes",
            validate::MAX_ENV_TOTAL_BYTES
        )));
    }
    Ok(())
}

/// A pending env change, for [`check_service_env`].
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvChange<'a> {
    /// The service's own variables after the change.
    pub own: Option<&'a [EnvVar]>,
    /// The variables of one linked env group after the change: (group id, vars).
    pub group: Option<(&'a str, &'a [EnvVar])>,
    /// The variables of an env group about to be linked (appended last).
    pub link: Option<&'a [EnvVar]>,
}

/// Check the combined environment `svc` would have after `change`.
pub async fn check_service_env(store: &Store, svc: &Service, change: EnvChange<'_>) -> Result<()> {
    let mut layers = Vec::new();
    for g in store.service_env_groups(&svc.id).await? {
        match change.group {
            Some((id, vars)) if id == g.id => layers.push(vars.to_vec()),
            _ => layers.push(store.list_env(&g.id).await?),
        }
    }
    if let Some(vars) = change.link {
        layers.push(vars.to_vec());
    }
    layers.push(match change.own {
        Some(vars) => vars.to_vec(),
        None => store.list_env(&svc.id).await?,
    });
    effective_env_size(&svc.name, &layers)
}

/// `current` with `unset` removed, then `set` upserted — the result of
/// `Store::patch_env`, computed up front so it can be validated.
pub fn patched_env(current: &[EnvVar], set: &[EnvVar], unset: &[String]) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = current.iter().filter(|v| !unset.contains(&v.key)).cloned().collect();
    for v in set {
        match out.iter_mut().find(|e| e.key == v.key) {
            Some(e) => e.value = v.value.clone(),
            None => out.push(v.clone()),
        }
    }
    out
}

/// The Dockerfile must stay inside the source directory: `dockerfile_path`
/// may use `..` (it is relative to `root_dir`), but `root_dir` joined with it
/// must not climb above the checkout. (The builder additionally rejects
/// symlinks that point outside.)
pub fn source_paths(svc: &Service) -> Result<()> {
    let Some(dockerfile) = &svc.dockerfile_path else { return Ok(()) };
    let segments = svc.root_dir.iter().flat_map(|r| r.split(['/', '\\'])).chain(dockerfile.split(['/', '\\']));
    let mut depth: usize = 0;
    for seg in segments {
        match seg {
            "" | "." => {}
            ".." => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::invalid(format!(
                        "invalid dockerfile_path '{dockerfile}': it points outside the source directory (it is relative to the root directory '{}')",
                        svc.root_dir.as_deref().unwrap_or(".")
                    ))
                })?;
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

/// What a `${{kind.name.property}}` reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Datastore,
    Service,
}

/// The `(kind, name)` of every reference in an env var value (same syntax
/// and kind aliases as `ferry_core::env`).
pub fn references(value: &str) -> Vec<(RefKind, String)> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else { break };
        let parts: Vec<&str> = after[..end].trim().split('.').map(str::trim).collect();
        if let [kind, name, _property] = parts.as_slice() {
            let kind = match kind.to_ascii_lowercase().as_str() {
                "datastore" | "db" | "database" => Some(RefKind::Datastore),
                "service" | "svc" => Some(RefKind::Service),
                _ => None,
            };
            if let Some(kind) = kind {
                out.push((kind, name.to_string()));
            }
        }
        rest = &after[end + 2..];
    }
    out
}

/// Services (other than `exclude`) whose environment references the
/// datastore or service `name`, as `service 'web' (DATABASE_URL, ...)`.
/// Deleting the target would make every later deploy / restart of them fail.
pub async fn referencing_services(
    store: &Store,
    kind: RefKind,
    name: &str,
    exclude: Option<&str>,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for svc in store.list_services().await? {
        if exclude == Some(svc.id.as_str()) {
            continue;
        }
        let keys: Vec<String> = store
            .effective_env(&svc.id)
            .await?
            .into_iter()
            .filter(|v| references(&v.value).iter().any(|(k, n)| *k == kind && n == name))
            .map(|v| v.key)
            .collect();
        if !keys.is_empty() {
            out.push(format!("service '{}' ({})", svc.name, keys.join(", ")));
        }
    }
    Ok(out)
}

/// Datastore version / image tag prefix: digits and dots only (`16`, `7.2`).
pub fn datastore_version(v: &str) -> Result<()> {
    let ok = !v.is_empty()
        && v.len() <= 16
        && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && !v.starts_with('.')
        && !v.ends_with('.')
        && !v.contains("..");
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid version '{v}': use digits and dots only (e.g. 16 or 7.2)")))
    }
}

/// Postgres database / user names: 1–63 chars of `[A-Za-z0-9_]`, not
/// starting with a digit (they end up in connection URLs and env vars).
pub fn pg_identifier(what: &str, v: &str) -> Result<()> {
    let ok = !v.is_empty()
        && v.len() <= 63
        && !v.as_bytes()[0].is_ascii_digit()
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!("invalid {what} '{v}': use 1-63 letters, digits or '_', not starting with a digit")))
    }
}

fn is_dashboard_host(config: &Config, host: &str) -> bool {
    config.dashboard_host.as_deref().is_some_and(|h| h.trim().trim_end_matches('.').eq_ignore_ascii_case(host))
}

/// Check that custom `domains` (already normalized) of the service named
/// `own_name` are free: not the dashboard host, not a default host
/// (`<name>.<base_domain>`) of any service, and not a custom domain of one of
/// the `others` (every service except this one).
pub fn check_domains<'a>(
    config: &Config,
    others: impl IntoIterator<Item = &'a Service> + Clone,
    own_name: &str,
    domains: &[String],
) -> Result<()> {
    for d in domains {
        if is_dashboard_host(config, d) {
            return Err(Error::conflict(format!("domain '{d}' is reserved for the Ferry dashboard")));
        }
        if *d == config.default_host(own_name) {
            return Err(Error::conflict(format!("domain '{d}' is already the default host of service '{own_name}'")));
        }
        for s in others.clone() {
            if *d == config.default_host(&s.name) {
                return Err(Error::conflict(format!("domain '{d}' is the default host of service '{}'", s.name)));
            }
            if s.custom_domains.iter().any(|c| c.eq_ignore_ascii_case(d)) {
                return Err(Error::conflict(format!("domain '{d}' is already used by service '{}'", s.name)));
            }
        }
    }
    Ok(())
}

/// Check that the default host of a new service named `name` isn't already
/// claimed as a custom domain by another service (or by the dashboard).
pub fn check_default_host_free<'a>(
    config: &Config,
    others: impl IntoIterator<Item = &'a Service>,
    name: &str,
) -> Result<()> {
    let host = config.default_host(name);
    if is_dashboard_host(config, &host) {
        return Err(Error::conflict(format!("host '{host}' is reserved for the Ferry dashboard; choose another name")));
    }
    for s in others {
        if s.custom_domains.iter().any(|c| c.eq_ignore_ascii_case(&host)) {
            return Err(Error::conflict(format!(
                "host '{host}' is already used as a custom domain by service '{}'",
                s.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::ServiceType;

    #[test]
    fn domain_rules() {
        let cfg = Config::default();
        let mut a = Service::new("a", ServiceType::WebService);
        a.custom_domains = vec!["app.example.com".into()];
        let others = [a];
        assert!(check_domains(&cfg, &others, "b", &["new.example.com".into()]).is_ok());
        assert!(matches!(check_domains(&cfg, &others, "b", &["app.example.com".into()]), Err(Error::Conflict(_))));
        assert!(matches!(check_domains(&cfg, &others, "b", &["a.localhost".into()]), Err(Error::Conflict(_))));
        assert!(matches!(check_domains(&cfg, &others, "b", &["b.localhost".into()]), Err(Error::Conflict(_))));
        assert!(matches!(check_domains(&cfg, &others, "b", &["ferry.localhost".into()]), Err(Error::Conflict(_))));
        assert!(check_default_host_free(&cfg, &others, "c").is_ok());
        let mut x = Service::new("x", ServiceType::WebService);
        x.custom_domains = vec!["c.localhost".into()];
        assert!(check_default_host_free(&cfg, &[x], "c").is_err());
    }

    #[test]
    fn env_sizes_and_patches() {
        let big = "x".repeat(validate::MAX_ENV_VALUE_BYTES - 100);
        let group: Vec<EnvVar> = (0..3).map(|i| EnvVar::new(format!("G{i}"), big.clone())).collect();
        let own: Vec<EnvVar> = (0..3).map(|i| EnvVar::new(format!("O{i}"), big.clone())).collect();
        assert!(effective_env_size("web", &[group.clone(), own.clone()]).is_ok());
        let group2: Vec<EnvVar> = (0..3).map(|i| EnvVar::new(format!("H{i}"), big.clone())).collect();
        let err = effective_env_size("web", &[group.clone(), group2, own]).unwrap_err().to_string();
        assert!(err.contains("service 'web'") && err.contains("maximum"), "{err}");
        // overridden keys count once
        assert!(effective_env_size("web", &[group.clone(), group.clone(), group]).is_ok());

        let cur = vec![EnvVar::new("A", "1"), EnvVar::new("B", "2")];
        let p = patched_env(&cur, &[EnvVar::new("B", "3"), EnvVar::new("C", "4")], &["A".into(), "C".into()]);
        assert_eq!(p, vec![EnvVar::new("B", "3"), EnvVar::new("C", "4")]);
    }

    #[test]
    fn dockerfile_must_stay_in_the_checkout() {
        let mut s = Service::new("web", ferry_core::ServiceType::WebService);
        for ok in [None, Some("Dockerfile"), Some("./docker/Dockerfile"), Some("a/../Dockerfile")] {
            s.dockerfile_path = ok.map(String::from);
            assert!(source_paths(&s).is_ok(), "{ok:?}");
        }
        s.dockerfile_path = Some("../../../../../etc/hosts".into());
        let err = source_paths(&s).unwrap_err().to_string();
        assert!(err.contains("outside the source directory"), "{err}");
        s.dockerfile_path = Some("../Dockerfile".into());
        assert!(source_paths(&s).is_err());
        s.root_dir = Some("services/api".into());
        assert!(source_paths(&s).is_ok());
        s.dockerfile_path = Some("../../../Dockerfile".into());
        assert!(source_paths(&s).is_err());
    }

    #[test]
    fn reference_scanning() {
        let refs = references(
            "postgres://${{ datastore.db.user }}@${{db.db.host}} ${{svc.api.hostport}} ${{bogus.x.y}} ${{broken",
        );
        assert_eq!(
            refs,
            vec![
                (RefKind::Datastore, "db".to_string()),
                (RefKind::Datastore, "db".to_string()),
                (RefKind::Service, "api".to_string())
            ]
        );
        assert!(references("plain value").is_empty());
    }

    #[test]
    fn datastore_rules() {
        assert!(datastore_version("16").is_ok());
        assert!(datastore_version("7.2").is_ok());
        for bad in ["", "16-alpine", "latest", ".1", "1.", "1..2"] {
            assert!(datastore_version(bad).is_err(), "{bad}");
        }
        assert!(pg_identifier("user", "app_user").is_ok());
        assert!(pg_identifier("user", "1app").is_err());
        assert!(pg_identifier("user", "app-user").is_err());
        assert!(pg_identifier("user", "").is_err());
    }
}
