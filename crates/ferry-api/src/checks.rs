//! Validation rules shared by the REST handlers and blueprints.

use ferry_core::{Config, EnvVar, Error, Result, Service, validate};

/// Validate env var keys (and reject NUL bytes in values, which no process
/// environment can hold).
pub fn validate_env_vars(vars: &[EnvVar]) -> Result<()> {
    for v in vars {
        validate::env_key(&v.key)?;
        if v.value.contains('\0') {
            return Err(Error::invalid(format!("environment variable '{}' contains a NUL byte", v.key)));
        }
    }
    Ok(())
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
        assert!(validate_env_vars(&[EnvVar::new("A", "1")]).is_ok());
        assert!(validate_env_vars(&[EnvVar::new("A B", "1")]).is_err());
        assert!(validate_env_vars(&[EnvVar::new("A", "x\0y")]).is_err());
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
