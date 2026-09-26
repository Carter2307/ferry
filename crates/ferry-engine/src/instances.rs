//! Everything needed to run a service instance: environment, port, command,
//! disk, the container spec, plus small container helpers.

use std::time::Duration;

use ferry_core::env::{self, RefContext};
use ferry_core::naming::{LABEL_INSTANCE, LABEL_ROLE, LABEL_SERVICE, ROLE_SERVICE};
use ferry_core::{EnvVar, Error, Naming, Result, Runtime, Service, ServiceType, Store, ids};
use ferry_docker::{ContainerInfo, ContainerSpec, PortPublish, RestartPolicy, VolumeMount};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::state::Inner;

/// Grace period when stopping old / surplus instances.
pub(crate) const STOP_GRACE_SECS: u32 = 10;
/// Host IP service ports are published on (the proxy and health checks
/// connect to `127.0.0.1:<host_port>`, never to container IPs).
pub(crate) const SERVICE_BIND_IP: &str = "127.0.0.1";

/// How a deploy's image was produced, kept so that restarts/rollbacks
/// (which reuse the image) apply the start command and port hint exactly
/// like the original build did. Stored in the `settings` table under
/// `engine.build.<deploy_id>`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BuildInfo {
    /// Runtime actually used (`docker`/`image` → the engine applies the start
    /// command; generated Dockerfiles bake it in).
    pub runtime: Option<Runtime>,
    /// Port the image is known to listen on (80 for generated static sites).
    pub port_hint: Option<u16>,
}

const BUILD_INFO_PREFIX: &str = "engine.build.";

fn build_info_key(deploy_id: &str) -> String {
    format!("{BUILD_INFO_PREFIX}{deploy_id}")
}

pub(crate) async fn save_build_info(store: &Store, deploy_id: &str, info: BuildInfo) -> Result<()> {
    let json = serde_json::to_string(&info).map_err(|e| Error::internal(format!("encoding build info: {e}")))?;
    store.set_setting(&build_info_key(deploy_id), &json).await
}

pub(crate) async fn load_build_info(store: &Store, deploy_id: &str) -> Option<BuildInfo> {
    match store.get_setting(&build_info_key(deploy_id)).await {
        Ok(Some(json)) => serde_json::from_str(&json).ok(),
        Ok(None) => None,
        Err(e) => {
            debug!(deploy = deploy_id, "cannot read build info: {e}");
            None
        }
    }
}

/// Forget what the engine stored about deploys (build info, launch spec):
/// their image is gone, or they were deleted. Best effort.
pub(crate) async fn forget_deploys(store: &Store, deploy_ids: &[String]) {
    for id in deploy_ids {
        if let Err(e) = store.delete_setting(&build_info_key(id)).await {
            debug!(deploy = %id, "cannot delete build info: {e}");
        }
    }
    crate::spec::forget(store, deploy_ids).await;
}

/// The container command for a service: `["/bin/sh","-c", start_command]`
/// when a start command is set and the image does not already run it —
/// images of the `docker`/`image` runtimes. Generated Dockerfiles bake the
/// start command in (and static sites run nginx), so they keep their CMD.
/// `runtime` is the runtime the image was built with, if known.
pub(crate) fn start_cmd(svc: &Service, runtime: Option<Runtime>) -> Option<Vec<String>> {
    let cmd = svc.start_command.as_deref().map(str::trim).filter(|c| !c.is_empty())?;
    let applies = match runtime {
        Some(Runtime::Docker | Runtime::Image) => true,
        Some(_) => false,
        // Unknown (build info lost): the command is equivalent to the baked-in
        // CMD of generated images, except for nginx-served static sites.
        None => svc.service_type != ServiceType::StaticSite && svc.runtime != Runtime::Static,
    };
    applies.then(|| sh_c(cmd))
}

/// `["/bin/sh", "-c", cmd]`.
pub(crate) fn sh_c(cmd: &str) -> Vec<String> {
    vec!["/bin/sh".to_string(), "-c".to_string(), cmd.to_string()]
}

/// Why `env::choose_port` picked what it picked (for the deploy log).
pub(crate) fn port_reason(
    service_port: Option<u16>,
    user_env: &[EnvVar],
    hint: Option<u16>,
    exposed: &[u16],
) -> &'static str {
    if service_port.is_some() {
        "service setting"
    } else if user_env.iter().any(|v| v.key == "PORT" && v.value.trim().parse::<u16>().is_ok_and(|p| p > 0)) {
        "PORT environment variable"
    } else if hint.is_some() {
        "detected from the build"
    } else if exposed.len() == 1 {
        "EXPOSEd by the image"
    } else {
        "default port"
    }
}

/// A warning when the user's `PORT` variable (which overrides the one Ferry
/// injects) disagrees with the port Ferry publishes and health-checks.
pub(crate) fn port_env_mismatch(port: u16, user_env: &[EnvVar]) -> Option<String> {
    let value = user_env.iter().rev().find(|v| v.key == "PORT")?.value.trim().to_string();
    if value.parse::<u16>().ok() == Some(port) {
        return None;
    }
    Some(format!(
        "the PORT environment variable is '{value}' but Ferry routes to container port {port}: \
         the app gets PORT={value} (remove the PORT variable or change the service port)"
    ))
}

/// Build args: the service's env with references resolved. Variables whose
/// references cannot be resolved (yet) are skipped; the second value lists
/// them with the reason.
pub(crate) async fn build_args(inner: &Inner, svc: &Service) -> Result<(Vec<(String, String)>, Vec<String>)> {
    let user = inner.store.effective_env(&svc.id).await?;
    let (datastores, services) = inner.store.reference_targets(&inner.config).await?;
    let ctx = RefContext { datastores: &datastores, services: &services, advertise_host: &inner.config.advertise_host };
    let mut args = Vec::with_capacity(user.len());
    let mut skipped = Vec::new();
    for v in &user {
        match env::resolve_value(&v.value, &ctx) {
            Ok(value) => args.push((v.key.clone(), value)),
            Err(e) => skipped.push(format!("{} ({})", v.key, crate::util::error_message(&e))),
        }
    }
    Ok((args, skipped))
}

/// The service's disk mounted at `mount_path` (creating the volume if needed).
pub(crate) async fn disk_volume(
    inner: &Inner,
    service_id: &str,
    mount_path: Option<&str>,
) -> Result<Option<VolumeMount>> {
    let Some(target) = mount_path.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let volume = inner.naming.service_volume(service_id);
    let mut labels = inner.naming.base_labels(ROLE_SERVICE);
    labels.insert(LABEL_SERVICE.to_string(), service_id.to_string());
    inner.docker.ensure_volume(&volume, &labels).await?;
    Ok(Some(VolumeMount { volume, target: target.to_string() }))
}

/// What every instance of one deploy is started with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    pub image: String,
    /// Container port (listening services only).
    pub port: Option<u16>,
    pub env: Vec<(String, String)>,
    pub cmd: Option<Vec<String>>,
    pub volume: Option<VolumeMount>,
}

/// Container spec of one instance.
pub(crate) fn service_spec(naming: &Naming, svc: &Service, deploy_id: &str, plan: &Plan) -> ContainerSpec {
    ContainerSpec {
        name: naming.service_container(&svc.name, deploy_id, &ids::random_secret(6)),
        image: plan.image.clone(),
        env: plan.env.clone(),
        cmd: plan.cmd.clone(),
        entrypoint: None,
        labels: naming.service_labels(&svc.id, deploy_id),
        network: Some(naming.network()),
        network_aliases: vec![svc.name.clone()],
        publish: plan.port.filter(|_| svc.listens()).map(|p| PortPublish {
            container_port: p,
            host_ip: SERVICE_BIND_IP.to_string(),
            host_port: None,
        }),
        volumes: plan.volume.iter().cloned().collect(),
        restart_policy: RestartPolicy::UnlessStopped,
        memory_limit_bytes: None,
        nano_cpus: None,
        working_dir: None,
    }
}

/// Containers of a service (role `service`), optionally including stopped ones.
pub(crate) async fn service_containers(
    inner: &Inner,
    service_id: &str,
    include_stopped: bool,
) -> Result<Vec<ContainerInfo>> {
    let prefix = inner.naming.prefix().to_string();
    let mut list = inner
        .docker
        .list_containers(
            &[(LABEL_INSTANCE, prefix.as_str()), (LABEL_ROLE, ROLE_SERVICE), (LABEL_SERVICE, service_id)],
            include_stopped,
        )
        .await?;
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}

/// Stop (with a grace period) and remove containers, in parallel. Every
/// container is attempted; the first error is returned.
pub(crate) async fn retire(inner: &Inner, containers: &[ContainerInfo], grace_secs: u32) -> Result<()> {
    let results = join_all(containers.iter().map(|c| async move {
        let stopped = if grace_secs > 0 && c.state.is_running() {
            inner.docker.stop_container(&c.id, grace_secs).await
        } else {
            Ok(())
        };
        if let Err(e) = &stopped {
            warn!(container = %c.name, "stopping failed, removing it anyway: {e}");
        }
        let removed = inner.docker.remove_container(&c.id, true).await;
        match &removed {
            Ok(()) => debug!(container = %c.name, "removed container"),
            Err(e) => warn!(container = %c.name, "removing container failed: {e}"),
        }
        removed
    }))
    .await;
    results.into_iter().find(|r| r.is_err()).unwrap_or(Ok(()))
}

/// Wait (bounded) until a volume can be removed: a volume stays "in use"
/// for a moment after its container is removed.
pub(crate) async fn remove_volume_retrying(inner: &Inner, volume: &str) -> Result<()> {
    let mut attempt = 0;
    loop {
        match inner.docker.remove_volume(volume).await {
            Err(Error::Conflict(_)) if attempt < 10 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{Deploy, DeploySource, DeployTrigger};

    fn web() -> Service {
        let mut s = Service::new("web", ServiceType::WebService);
        s.id = "srv-0123456789abcdef0123".into();
        s
    }

    #[test]
    fn start_command_rules() {
        let mut s = web();
        assert_eq!(start_cmd(&s, Some(Runtime::Docker)), None, "no start command");
        s.start_command = Some("  ./run --port $PORT ".into());
        let sh = Some(sh_c("./run --port $PORT"));
        assert_eq!(start_cmd(&s, Some(Runtime::Docker)), sh);
        assert_eq!(start_cmd(&s, Some(Runtime::Image)), sh);
        assert_eq!(start_cmd(&s, Some(Runtime::Node)), None, "baked into the generated Dockerfile");
        assert_eq!(start_cmd(&s, Some(Runtime::Static)), None);
        assert_eq!(start_cmd(&s, None), sh);
        s.service_type = ServiceType::StaticSite;
        assert_eq!(start_cmd(&s, None), None);
        s.start_command = Some("   ".into());
        assert_eq!(start_cmd(&s, Some(Runtime::Docker)), None);
    }

    #[test]
    fn port_reasons_follow_choose_port_priority() {
        let port_env = vec![EnvVar::new("PORT", "4000")];
        assert_eq!(port_reason(Some(1), &port_env, Some(80), &[3000]), "service setting");
        assert_eq!(port_reason(None, &port_env, Some(80), &[3000]), "PORT environment variable");
        assert_eq!(port_reason(None, &[EnvVar::new("PORT", "x")], Some(80), &[]), "detected from the build");
        assert_eq!(port_reason(None, &[], None, &[3000]), "EXPOSEd by the image");
        assert_eq!(port_reason(None, &[], None, &[3000, 3001]), "default port");
        assert_eq!(env::choose_port(None, &[], None, &[3000, 3001], 10000), 10000);
    }

    #[test]
    fn port_env_mismatches_are_reported() {
        assert_eq!(port_env_mismatch(8000, &[]), None);
        assert_eq!(port_env_mismatch(8000, &[EnvVar::new("PORT", " 8000 ")]), None);
        let w = port_env_mismatch(8000, &[EnvVar::new("PORT", "9000")]).unwrap();
        assert!(w.contains("'9000'") && w.contains("8000"), "{w}");
        assert!(port_env_mismatch(8000, &[EnvVar::new("PORT", "abc")]).is_some());
    }

    #[test]
    fn spec_for_listening_service() {
        let naming = Naming::new("ferry");
        let svc = web();
        let deploy_id = "dep-0123456789abcdef4567";
        let plan = Plan {
            image: "ferry/web:dep-0123456789abcdef4567".into(),
            port: Some(8000),
            env: vec![("PORT".into(), "8000".into())],
            cmd: Some(sh_c("python app.py")),
            volume: Some(VolumeMount { volume: "ferry-svc-x-disk".into(), target: "/data".into() }),
        };
        let spec = service_spec(&naming, &svc, deploy_id, &plan);
        assert!(spec.name.starts_with("ferry-web-cdef4567-"), "{}", spec.name);
        assert_eq!(spec.name.len(), "ferry-web-cdef4567-".len() + 6);
        assert_eq!(spec.image, plan.image);
        assert_eq!(spec.network.as_deref(), Some("ferry"));
        assert_eq!(spec.network_aliases, vec!["web"]);
        assert_eq!(
            spec.publish,
            Some(PortPublish { container_port: 8000, host_ip: "127.0.0.1".into(), host_port: None })
        );
        assert_eq!(spec.restart_policy, RestartPolicy::UnlessStopped);
        assert_eq!(spec.labels["ferry.deploy"], deploy_id);
        assert_eq!(spec.labels["ferry.service"], svc.id);
        assert_eq!(spec.labels["ferry.role"], "service");
        assert_eq!(spec.volumes.len(), 1);
        assert_eq!(spec.cmd, plan.cmd);

        // Workers don't publish anything even if a port is known.
        let mut worker = web();
        worker.service_type = ServiceType::BackgroundWorker;
        assert!(service_spec(&naming, &worker, deploy_id, &plan).publish.is_none());
    }

    #[tokio::test]
    async fn env_injection_and_user_override() {
        let store = Store::open_in_memory().await.unwrap();
        let mut svc = web();
        svc.name = "api".into();
        store.create_service(&svc).await.unwrap();
        let mut ds = ferry_core::Datastore::new("cache", ferry_core::DatastoreKind::Redis);
        ds.password = "pw".into();
        store.create_datastore(&ds).await.unwrap();
        store.set_env(&svc.id, "REDIS_URL", "${{datastore.cache.connectionString}}").await.unwrap();
        store.set_env(&svc.id, "FERRY_SERVICE_NAME", "custom").await.unwrap();
        store.set_env(&svc.id, "BROKEN", "${{datastore.nope.host}}").await.unwrap();
        let group = ferry_core::EnvGroup::new("shared");
        store.create_env_group(&group).await.unwrap();
        store.replace_env(&group.id, &[EnvVar::new("SHARED", "1"), EnvVar::new("REDIS_URL", "group")]).await.unwrap();
        store.link_env_group(&svc.id, &group.id).await.unwrap();

        let config = ferry_core::Config::default();
        let (datastores, services) = store.reference_targets(&config).await.unwrap();
        let ctx = RefContext { datastores: &datastores, services: &services, advertise_host: "127.0.0.1" };
        let user = store.effective_env(&svc.id).await.unwrap();
        // Unresolvable references fail the whole resolution (at container start)...
        assert!(env::resolve_all(&user, &ctx).is_err());
        store.unset_env(&svc.id, "BROKEN").await.unwrap();
        let user = store.effective_env(&svc.id).await.unwrap();
        let resolved = env::resolve_all(&user, &ctx).unwrap();
        let deploy = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        let merged = env::container_env(env::injected(&svc, &deploy, Some(8000), &config), resolved);
        let get = |k: &str| merged.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("PORT").as_deref(), Some("8000"));
        assert_eq!(get("REDIS_URL").as_deref(), Some("redis://default:pw@cache:6379"), "service overrides group");
        assert_eq!(get("SHARED").as_deref(), Some("1"));
        assert_eq!(get("FERRY_SERVICE_NAME").as_deref(), Some("custom"), "user vars override injected ones");
        assert_eq!(get("FERRY_DEPLOY_ID"), Some(deploy.id.clone()));
        assert_eq!(get("FERRY_EXTERNAL_URL").as_deref(), Some("http://api.localhost:8080"));
    }

    #[tokio::test]
    async fn build_info_roundtrip() {
        let store = Store::open_in_memory().await.unwrap();
        assert_eq!(load_build_info(&store, "dep-1").await, None);
        let info = BuildInfo { runtime: Some(Runtime::Static), port_hint: Some(80) };
        save_build_info(&store, "dep-1", info).await.unwrap();
        assert_eq!(load_build_info(&store, "dep-1").await, Some(info));
        forget_deploys(&store, &["dep-1".to_string()]).await;
        assert_eq!(load_build_info(&store, "dep-1").await, None);
    }
}
