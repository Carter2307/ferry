//! Launch specs: exactly what a deploy's instances (and one-off / cron jobs)
//! run with — image, container port, resolved environment, command and
//! disk. A deploy computes its spec from the service's settings and env when
//! it starts its instances, and stores it (JSON in the `settings` table under
//! `engine.spec.<deploy_id>`) when it goes live.
//!
//! Everything that starts containers for an existing deploy — the reconciler
//! replacing a crashed instance, scale, resume, jobs — uses the live deploy's
//! spec, never the current settings: changed settings and env only apply
//! with the next deploy or restart (like Render). So a failed deploy never
//! changes what the live one runs, and all instances of a deploy are alike.

use ferry_core::env::{self, RefContext};
use ferry_core::{Config, Deploy, EnvVar, Error, Result, Runtime, Service, Store};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::instances::{self, BuildInfo, Plan};
use crate::state::Inner;
use crate::util::error_message;

const SPEC_PREFIX: &str = "engine.spec.";

/// What every instance of one deploy is started with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LaunchSpec {
    pub image: String,
    /// Container port (listening services only).
    pub port: Option<u16>,
    /// Ferry's injected variables (`PORT` included for listening services).
    pub injected_env: Vec<EnvVar>,
    /// The service's env (groups merged) with references resolved.
    pub user_env: Vec<EnvVar>,
    /// Container command of the instances (`None` = the image's CMD).
    pub cmd: Option<Vec<String>>,
    /// The service's start command at deploy time (the default command of
    /// cron runs).
    pub start_command: Option<String>,
    /// Where the service's disk is mounted, if it has one.
    pub disk_mount_path: Option<String>,
}

impl LaunchSpec {
    /// The spec of a new deploy (or restart) from the service's current
    /// settings. `user_env` is already resolved (see [`resolve_user_env`]).
    pub(crate) fn new(
        svc: &Service,
        deploy: &Deploy,
        image: String,
        port: Option<u16>,
        runtime: Option<Runtime>,
        user_env: Vec<EnvVar>,
        config: &Config,
    ) -> Self {
        LaunchSpec {
            image,
            port,
            injected_env: env::injected(svc, deploy, port, config),
            user_env,
            cmd: instances::start_cmd(svc, runtime),
            start_command: svc.start_command.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string),
            disk_mount_path: svc
                .disk_mount_path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string),
        }
    }

    /// The instances' environment: injected variables overridden by the
    /// user's.
    pub(crate) fn container_env(&self) -> Vec<(String, String)> {
        env::container_env(self.injected_env.clone(), self.user_env.clone())
    }

    /// A job's environment: like the service, but no injected `PORT`
    /// (nothing listens in a job).
    pub(crate) fn job_env(&self) -> Vec<(String, String)> {
        let injected = self.injected_env.iter().filter(|v| v.key != "PORT").cloned().collect();
        env::container_env(injected, self.user_env.clone())
    }
}

fn key(deploy_id: &str) -> String {
    format!("{SPEC_PREFIX}{deploy_id}")
}

pub(crate) async fn save(store: &Store, deploy_id: &str, spec: &LaunchSpec) -> Result<()> {
    let json = serde_json::to_string(spec).map_err(|e| Error::internal(format!("encoding launch spec: {e}")))?;
    store.set_setting(&key(deploy_id), &json).await
}

pub(crate) async fn load(store: &Store, deploy_id: &str) -> Result<Option<LaunchSpec>> {
    match store.get_setting(&key(deploy_id)).await? {
        Some(json) => match serde_json::from_str(&json) {
            Ok(spec) => Ok(Some(spec)),
            Err(e) => {
                debug!(deploy = deploy_id, "ignoring an unreadable launch spec: {e}");
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

/// Forget the spec of deploys (best effort).
pub(crate) async fn forget(store: &Store, deploy_ids: &[String]) {
    for id in deploy_ids {
        if let Err(e) = store.delete_setting(&key(id)).await {
            debug!(deploy = %id, "cannot delete launch spec: {e}");
        }
    }
}

/// The service's effective env with every `${{…}}` reference resolved.
/// Unresolvable references are an error.
pub(crate) async fn resolve_user_env(inner: &Inner, svc: &Service) -> Result<Vec<EnvVar>> {
    let resolved = async {
        let user = inner.store.effective_env(&svc.id).await?;
        let (datastores, services) = inner.store.reference_targets(&inner.config).await?;
        let ctx =
            RefContext { datastores: &datastores, services: &services, advertise_host: &inner.config.advertise_host };
        env::resolve_all(&user, &ctx)
    }
    .await;
    resolved.map_err(|e| Error::invalid(format!("cannot resolve environment variables: {}", error_message(&e))))
}

/// The spec of an existing deploy (reconciler, scale, resume, jobs): its
/// stored snapshot. Deploys that went live before snapshots existed get one
/// computed from the current settings, which is then stored so that their
/// instances stay alike from now on.
pub(crate) async fn for_deploy(inner: &Inner, svc: &Service, deploy: &Deploy) -> Result<LaunchSpec> {
    if let Some(spec) = load(&inner.store, &deploy.id).await? {
        return Ok(spec);
    }
    let image = deploy.image.clone().ok_or_else(|| Error::conflict(format!("deploy {} has no image", deploy.id)))?;
    let info: Option<BuildInfo> = instances::load_build_info(&inner.store, &deploy.id).await;
    let user_env = resolve_user_env(inner, svc).await?;
    let port = if svc.listens() {
        match deploy.port {
            Some(p) => Some(p),
            None => {
                let exposed = inner.docker.image_exposed_ports(&image).await.unwrap_or_default();
                let hint = info.and_then(|i| i.port_hint);
                Some(env::choose_port(svc.port, &user_env, hint, &exposed, inner.config.default_port))
            }
        }
    } else {
        None
    };
    let spec = LaunchSpec::new(svc, deploy, image, port, info.and_then(|i| i.runtime), user_env, &inner.config);
    save(&inner.store, &deploy.id, &spec).await?;
    info!(service = %svc.name, deploy = %deploy.id, "recorded the launch spec of a deploy made by an older version");
    Ok(spec)
}

/// The container plan of a spec (creating the disk volume if needed).
pub(crate) async fn plan(inner: &Inner, svc: &Service, spec: &LaunchSpec) -> Result<Plan> {
    let volume = instances::disk_volume(inner, &svc.id, spec.disk_mount_path.as_deref()).await?;
    Ok(Plan { image: spec.image.clone(), port: spec.port, env: spec.container_env(), cmd: spec.cmd.clone(), volume })
}

/// Names of the services whose port is referenced in the values of `vars`
/// (`${{service.NAME.port}}`, `hostport` or `internalUrl`; `svc.` too):
/// those references resolve only once the service's port is known.
pub(crate) fn port_referenced_services(vars: &[EnvVar]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for v in vars {
        let mut rest = v.value.as_str();
        while let Some(start) = rest.find("${{") {
            let after = &rest[start + 3..];
            let Some(end) = after.find("}}") else { break };
            let parts: Vec<&str> = after[..end].split('.').map(str::trim).collect();
            if let [kind, name, prop] = parts.as_slice()
                && matches!(kind.to_ascii_lowercase().as_str(), "service" | "svc")
                && matches!(*prop, "port" | "hostport" | "internalUrl")
                && !names.iter().any(|n| n == name)
            {
                names.push((*name).to_string());
            }
            rest = &after[end + 2..];
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{DeploySource, DeployTrigger, ServiceType};

    #[test]
    fn spec_env_command_and_disk() {
        let config = Config::default();
        let mut svc = Service::new("web", ServiceType::WebService);
        svc.start_command = Some("  ./serve  ".into());
        svc.disk_mount_path = Some("/data".into());
        let deploy = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        let user = vec![EnvVar::new("GREETING", "hi"), EnvVar::new("FERRY_SERVICE_NAME", "custom")];
        let spec =
            LaunchSpec::new(&svc, &deploy, "img".into(), Some(8000), Some(Runtime::Docker), user.clone(), &config);
        assert_eq!(spec.cmd, Some(instances::sh_c("./serve")));
        assert_eq!(spec.start_command.as_deref(), Some("./serve"));
        assert_eq!(spec.disk_mount_path.as_deref(), Some("/data"));
        let get = |env: &[(String, String)], k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        let env = spec.container_env();
        assert_eq!(get(&env, "PORT").as_deref(), Some("8000"));
        assert_eq!(get(&env, "GREETING").as_deref(), Some("hi"));
        assert_eq!(get(&env, "FERRY_SERVICE_NAME").as_deref(), Some("custom"), "user vars win");
        assert_eq!(get(&env, "FERRY_DEPLOY_ID"), Some(deploy.id.clone()));
        let jobs = spec.job_env();
        assert_eq!(get(&jobs, "PORT"), None, "jobs get no injected PORT");
        assert_eq!(get(&jobs, "GREETING").as_deref(), Some("hi"));

        // A PORT the user set is kept for jobs (it is their variable).
        let with_port =
            LaunchSpec::new(&svc, &deploy, "img".into(), Some(8000), None, vec![EnvVar::new("PORT", "9000")], &config);
        assert_eq!(get(&with_port.job_env(), "PORT").as_deref(), Some("9000"));
        // Native runtimes bake the start command into the image.
        let native = LaunchSpec::new(&svc, &deploy, "img".into(), None, Some(Runtime::Node), user, &config);
        assert_eq!(native.cmd, None);
        assert_eq!(native.start_command.as_deref(), Some("./serve"), "still the default of cron runs");
    }

    #[tokio::test]
    async fn specs_roundtrip_through_the_store() {
        let store = Store::open_in_memory().await.unwrap();
        let svc = Service::new("web", ServiceType::WebService);
        let deploy = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        let spec = LaunchSpec::new(
            &svc,
            &deploy,
            "img".into(),
            Some(80),
            None,
            vec![EnvVar::new("A", "1")],
            &Config::default(),
        );
        assert_eq!(load(&store, &deploy.id).await.unwrap(), None);
        save(&store, &deploy.id, &spec).await.unwrap();
        assert_eq!(load(&store, &deploy.id).await.unwrap(), Some(spec));
        forget(&store, std::slice::from_ref(&deploy.id)).await;
        assert_eq!(load(&store, &deploy.id).await.unwrap(), None);
        // Garbage is ignored, not an error.
        store.set_setting(&key("dep-bad"), "not json").await.unwrap();
        assert_eq!(load(&store, "dep-bad").await.unwrap(), None);
    }

    #[test]
    fn finds_port_references_to_services() {
        let vars = vec![
            EnvVar::new("A", "${{service.api.hostport}}"),
            EnvVar::new("B", "http://${{ svc.worker.port }}/x and ${{service.api.port}}"),
            EnvVar::new("C", "${{datastore.db.connectionString}} ${{service.broken"),
            EnvVar::new("D", "plain"),
            // Known without a deploy: no need to wait for these services.
            EnvVar::new("E", "${{service.web.url}} ${{service.web.host}}"),
            EnvVar::new("F", "${{service.internal.internalUrl}}"),
        ];
        assert_eq!(
            port_referenced_services(&vars),
            vec!["api".to_string(), "worker".to_string(), "internal".to_string()]
        );
    }
}
