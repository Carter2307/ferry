//! Environment variables: merging, reference resolution, injected variables
//! and port selection.
//!
//! ## References
//! Values may embed `${{ <kind>.<name>.<property> }}` references, resolved at
//! container start (so rotating a datastore password only needs a restart):
//!
//! | kind        | properties                                                                 |
//! |-------------|-----------------------------------------------------------------------------|
//! | `datastore` | `connectionString` (private network), `externalConnectionString`, `host`, `port`, `user`, `password`, `database` |
//! | `service`   | `host`, `port`, `hostport` (`host:port`), `url` (public URL), `internalUrl` (`http://host:port`) |
//!
//! `db`/`database` are aliases for `datastore`, `svc` for `service`.
//! Example: `DATABASE_URL=${{datastore.main-db.connectionString}}`.

use crate::config::Config;
use crate::models::{Datastore, Deploy, EnvVar, Service};
use crate::{Error, Result};

/// What a service reference can resolve to.
#[derive(Debug, Clone)]
pub struct ServiceRef {
    pub name: String,
    /// The port the service listens on (as chosen at its last deploy), if known.
    pub port: Option<u16>,
    /// Public URL (web services / static sites).
    pub public_url: Option<String>,
}

/// Everything needed to resolve references.
#[derive(Debug, Clone, Copy)]
pub struct RefContext<'a> {
    pub datastores: &'a [Datastore],
    pub services: &'a [ServiceRef],
    /// Host used in `externalConnectionString`.
    pub advertise_host: &'a str,
}

/// Merge env layers; later layers override earlier ones (keys keep the order
/// of first appearance). Typical order: env groups (in link order), then the
/// service's own variables.
pub fn merge(layers: &[Vec<EnvVar>]) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = Vec::new();
    for layer in layers {
        for v in layer {
            if let Some(existing) = out.iter_mut().find(|e| e.key == v.key) {
                existing.value = v.value.clone();
            } else {
                out.push(v.clone());
            }
        }
    }
    out
}

/// Replace every `${{kind.name.prop}}` in `value`. Unknown kinds, names or
/// properties are errors (so a typo fails the deploy loudly).
pub fn resolve_value(value: &str, ctx: &RefContext<'_>) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else {
            return Err(Error::invalid(format!("unterminated reference in '{value}'")));
        };
        let expr = after[..end].trim();
        out.push_str(&resolve_reference(expr, ctx)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve_reference(expr: &str, ctx: &RefContext<'_>) -> Result<String> {
    let parts: Vec<&str> = expr.split('.').map(str::trim).collect();
    let [kind, name, prop] = parts.as_slice() else {
        return Err(Error::invalid(format!(
            "invalid reference '${{{{{expr}}}}}': expected ${{{{kind.name.property}}}}"
        )));
    };
    match kind.to_ascii_lowercase().as_str() {
        "datastore" | "db" | "database" => {
            let ds = ctx
                .datastores
                .iter()
                .find(|d| d.name == *name)
                .ok_or_else(|| Error::invalid(format!("reference to unknown datastore '{name}'")))?;
            match *prop {
                "connectionString" | "url" | "internalUrl" => Ok(ds.internal_url()),
                "externalConnectionString" | "externalUrl" => ds
                    .external_url(ctx.advertise_host)
                    .ok_or_else(|| Error::invalid(format!("datastore '{name}' has no external port yet"))),
                "host" | "hostname" => Ok(ds.internal_host().to_string()),
                "port" => Ok(ds.internal_port().to_string()),
                "user" | "username" => Ok(ds.username.clone()),
                "password" => Ok(ds.password.clone()),
                "database" | "databaseName" => ds
                    .database
                    .clone()
                    .ok_or_else(|| Error::invalid(format!("datastore '{name}' has no database name"))),
                other => Err(Error::invalid(format!("unknown datastore property '{other}'"))),
            }
        }
        "service" | "svc" => {
            let svc = ctx
                .services
                .iter()
                .find(|s| s.name == *name)
                .ok_or_else(|| Error::invalid(format!("reference to unknown service '{name}'")))?;
            let port = || {
                svc.port.ok_or_else(|| Error::invalid(format!("service '{name}' has no known port (deploy it first)")))
            };
            match *prop {
                "host" | "hostname" => Ok(svc.name.clone()),
                "port" => Ok(port()?.to_string()),
                "hostport" => Ok(format!("{}:{}", svc.name, port()?)),
                "internalUrl" => Ok(format!("http://{}:{}", svc.name, port()?)),
                "url" | "externalUrl" => {
                    svc.public_url.clone().ok_or_else(|| Error::invalid(format!("service '{name}' has no public URL")))
                }
                other => Err(Error::invalid(format!("unknown service property '{other}'"))),
            }
        }
        other => Err(Error::invalid(format!("unknown reference kind '{other}' (use datastore or service)"))),
    }
}

/// Resolve references in every value.
pub fn resolve_all(vars: &[EnvVar], ctx: &RefContext<'_>) -> Result<Vec<EnvVar>> {
    vars.iter()
        .map(|v| {
            resolve_value(&v.value, ctx)
                .map(|value| EnvVar { key: v.key.clone(), value })
                .map_err(|e| Error::invalid(format!("env var {}: {e}", v.key)))
        })
        .collect()
}

/// Pick the container port. Priority: explicit `service.port` › user `PORT`
/// env var › build hint (e.g. 80 for static sites served by nginx) › the
/// image's single EXPOSEd port › `default_port`.
pub fn choose_port(
    service_port: Option<u16>,
    user_env: &[EnvVar],
    build_hint: Option<u16>,
    exposed: &[u16],
    default_port: u16,
) -> u16 {
    if let Some(p) = service_port {
        return p;
    }
    if let Some(p) =
        user_env.iter().find(|v| v.key == "PORT").and_then(|v| v.value.trim().parse::<u16>().ok()).filter(|p| *p > 0)
    {
        return p;
    }
    if let Some(p) = build_hint {
        return p;
    }
    if exposed.len() == 1 {
        return exposed[0];
    }
    default_port
}

/// Variables Ferry injects into every container (user variables override them).
pub fn injected(service: &Service, deploy: &Deploy, port: Option<u16>, config: &Config) -> Vec<EnvVar> {
    let mut v = vec![
        EnvVar::new("FERRY", "true"),
        EnvVar::new("FERRY_SERVICE_ID", &service.id),
        EnvVar::new("FERRY_SERVICE_NAME", &service.name),
        EnvVar::new("FERRY_SERVICE_TYPE", service.service_type.as_str()),
        EnvVar::new("FERRY_DEPLOY_ID", &deploy.id),
        EnvVar::new("FERRY_INTERNAL_HOSTNAME", &service.name),
    ];
    if let Some(p) = port {
        v.push(EnvVar::new("PORT", p.to_string()));
    }
    if let Some(sha) = &deploy.commit_sha {
        v.push(EnvVar::new("FERRY_GIT_COMMIT", sha));
        v.push(EnvVar::new("FERRY_GIT_BRANCH", &service.branch));
    }
    if let Some(url) = config.service_url(service) {
        v.push(EnvVar::new("FERRY_EXTERNAL_HOSTNAME", config.default_host(&service.name)));
        v.push(EnvVar::new("FERRY_EXTERNAL_URL", url));
    }
    v
}

/// Final container environment: injected vars overridden by the user's
/// (already merged and resolved) vars.
pub fn container_env(injected: Vec<EnvVar>, user_resolved: Vec<EnvVar>) -> Vec<(String, String)> {
    merge(&[injected, user_resolved]).into_iter().map(|v| (v.key, v.value)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DatastoreKind, DeploySource, DeployTrigger, ServiceType};

    fn ctx_fixture() -> (Vec<Datastore>, Vec<ServiceRef>) {
        let mut ds = Datastore::new("db", DatastoreKind::Postgres);
        ds.password = "pw".into();
        ds.host_port = Some(5555);
        let svcs = vec![ServiceRef { name: "api".into(), port: Some(3000), public_url: None }];
        (vec![ds], svcs)
    }

    #[test]
    fn resolves_references() {
        let (ds, svcs) = ctx_fixture();
        let ctx = RefContext { datastores: &ds, services: &svcs, advertise_host: "127.0.0.1" };
        assert_eq!(resolve_value("${{datastore.db.connectionString}}", &ctx).unwrap(), "postgresql://db:pw@db:5432/db");
        assert_eq!(
            resolve_value("x=${{ db.db.externalConnectionString }};", &ctx).unwrap(),
            "x=postgresql://db:pw@127.0.0.1:5555/db;"
        );
        assert_eq!(resolve_value("http://${{service.api.hostport}}/v1", &ctx).unwrap(), "http://api:3000/v1");
        assert_eq!(resolve_value("plain $HOME ${notref}", &ctx).unwrap(), "plain $HOME ${notref}");
        assert!(resolve_value("${{datastore.nope.host}}", &ctx).is_err());
        assert!(resolve_value("${{service.api.url}}", &ctx).is_err());
        assert!(resolve_value("${{service.api.port", &ctx).is_err());
    }

    #[test]
    fn merge_and_port() {
        let merged = merge(&[
            vec![EnvVar::new("A", "1"), EnvVar::new("B", "1")],
            vec![EnvVar::new("B", "2"), EnvVar::new("C", "3")],
        ]);
        assert_eq!(merged, vec![EnvVar::new("A", "1"), EnvVar::new("B", "2"), EnvVar::new("C", "3")]);
        assert_eq!(choose_port(Some(1), &[], Some(80), &[3000], 10000), 1);
        assert_eq!(choose_port(None, &[EnvVar::new("PORT", "4000")], Some(80), &[], 10000), 4000);
        assert_eq!(choose_port(None, &[], Some(80), &[3000], 10000), 80);
        assert_eq!(choose_port(None, &[], None, &[3000], 10000), 3000);
        assert_eq!(choose_port(None, &[], None, &[3000, 3001], 10000), 10000);
    }

    #[test]
    fn injected_vars() {
        let cfg = Config::default();
        let svc = Service::new("web", ServiceType::WebService);
        let dep = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        let env = container_env(injected(&svc, &dep, Some(10000), &cfg), vec![EnvVar::new("PORT", "8000")]);
        assert!(env.contains(&("PORT".to_string(), "8000".to_string())));
        assert!(env.iter().any(|(k, v)| k == "FERRY_EXTERNAL_URL" && v == "http://web.localhost:8080"));
    }
}
