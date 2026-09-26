//! Small side-effecting helpers shared by handlers and blueprints.

use std::collections::{BTreeMap, HashMap};

use ferry_core::{
    Datastore, DatastoreStatus, Deploy, DeployTrigger, Engine, EnvVar, Error, Result, Service, ServiceType, Store,
};

/// A set of variables as a key → value map: the order of variables doesn't
/// change what a container sees.
pub fn env_map(vars: &[EnvVar]) -> BTreeMap<&str, &str> {
    vars.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect()
}

/// Same variables (ignoring order).
pub fn same_env(a: &[EnvVar], b: &[EnvVar]) -> bool {
    env_map(a) == env_map(b)
}

/// The effective environments of some services (linked env groups + own
/// variables), captured before an env change so that only the services whose
/// environment really changed get restarted: rewriting identical values,
/// unsetting keys that don't exist or changing a group variable a service
/// overrides must not restart anything.
#[derive(Debug, Default)]
pub struct EnvSnapshot(HashMap<String, Vec<EnvVar>>);

impl EnvSnapshot {
    /// Capture the effective environment of every service in `service_ids`.
    pub async fn capture(store: &Store, service_ids: &[String]) -> Result<Self> {
        let mut map = HashMap::new();
        for id in service_ids {
            if !map.contains_key(id) {
                map.insert(id.clone(), store.effective_env(id).await?);
            }
        }
        Ok(EnvSnapshot(map))
    }

    /// [`capture`](Self::capture) for service rows.
    pub async fn capture_services(store: &Store, services: &[Service]) -> Result<Self> {
        let ids: Vec<String> = services.iter().map(|s| s.id.clone()).collect();
        Self::capture(store, &ids).await
    }

    /// Whether the service's effective environment differs from the captured
    /// one (a service that wasn't captured counts as changed).
    pub async fn changed(&self, store: &Store, service_id: &str) -> Result<bool> {
        match self.0.get(service_id) {
            Some(before) => Ok(!same_env(before, &store.effective_env(service_id).await?)),
            None => Ok(true),
        }
    }

    /// The services among `services` whose effective environment changed.
    /// Failures to read are logged and count as changed (restarting once too
    /// often beats keeping stale values).
    pub async fn changed_services(&self, store: &Store, services: &[Service]) -> Vec<Service> {
        let mut out = Vec::new();
        for svc in services {
            match self.changed(store, &svc.id).await {
                Ok(false) => tracing::debug!(service = %svc.name, "environment unchanged; not restarting"),
                Ok(true) => out.push(svc.clone()),
                Err(e) => {
                    tracing::warn!(service = %svc.name, "comparing the environment failed: {e}");
                    out.push(svc.clone());
                }
            }
        }
        out
    }
}

/// Restart a service so it picks up changed env vars. See [`restart_if_deployed`].
pub async fn restart_for_env_change(store: &Store, engine: &dyn Engine, service: &Service) -> Result<Option<Deploy>> {
    restart_if_deployed(store, engine, service, DeployTrigger::EnvChange).await
}

/// Restart a service so it picks up changed env vars or settings that only
/// apply through a deploy snapshot (e.g. a cron job's command). That's needed
/// when it has a live deploy, and also while a deploy is in flight (queued,
/// building or deploying): that deploy read — or will read — the values
/// before the change, so the engine queues the restart behind it. Suspended
/// services and services that were never deployed are left alone. Returns
/// the queued deploy, if any.
pub async fn restart_if_deployed(
    store: &Store,
    engine: &dyn Engine,
    service: &Service,
    trigger: DeployTrigger,
) -> Result<Option<Deploy>> {
    if service.suspended {
        return Ok(None);
    }
    if service.live_deploy_id.is_none() && !has_active_deploy(store, &service.id).await? {
        return Ok(None);
    }
    let deploy = engine.restart(&service.id, trigger).await?;
    tracing::info!(service = %service.name, deploy = %deploy.id, %trigger, "queued a restart");
    Ok(Some(deploy))
}

/// A cron job's command changed. Its runs use the command of the live
/// deploy's snapshot (not the image's), so the change applies through a
/// restart — without one it would never run.
pub fn cron_command_changed(old: &Service, new: &Service) -> bool {
    old.service_type == ServiceType::CronJob
        && new.service_type == ServiceType::CronJob
        && old.start_command != new.start_command
}

/// True while a deploy of the service is queued, building or deploying.
pub async fn has_active_deploy(store: &Store, service_id: &str) -> Result<bool> {
    Ok(store.active_deploys().await?.iter().any(|d| d.service_id == service_id))
}

/// [`restart_for_env_change`] for several services, logging (not returning)
/// per-service failures. Returns the queued deploys.
pub async fn restart_services(store: &Store, engine: &dyn Engine, services: &[Service]) -> Vec<Deploy> {
    let mut deploys = Vec::new();
    for svc in services {
        // Re-read: the row may have changed (suspended, deployed) meanwhile.
        let fresh = match store.get_service(&svc.id).await {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(service = %svc.name, "restart after env group change failed: {e}");
                continue;
            }
        };
        match restart_for_env_change(store, engine, &fresh).await {
            Ok(Some(d)) => deploys.push(d),
            Ok(None) => {}
            Err(e) => tracing::warn!(service = %svc.name, "restart after env group change failed: {e}"),
        }
    }
    deploys
}

/// Restart the services linked to an env group whose effective environment
/// changed since `before` (see [`restart_services`]).
pub async fn restart_group_services(
    store: &Store,
    engine: &dyn Engine,
    group_id: &str,
    before: &EnvSnapshot,
) -> Result<Vec<Deploy>> {
    let services = store.env_group_services(group_id).await?;
    let changed = before.changed_services(store, &services).await;
    Ok(restart_services(store, engine, &changed).await)
}

/// Record a synchronous provisioning failure on the datastore row.
pub async fn mark_datastore_failed(store: &Store, datastore_id: &str, err: &Error) -> Option<Datastore> {
    let res = async {
        let mut ds = store.require_datastore(datastore_id).await?;
        ds.status = DatastoreStatus::Failed;
        ds.error = Some(err.to_string());
        store.update_datastore(&ds).await
    }
    .await;
    match res {
        Ok(ds) => Some(ds),
        Err(e) => {
            tracing::warn!(datastore = datastore_id, "could not record provisioning failure: {e}");
            None
        }
    }
}

/// Ask the engine to re-read routes after a domain change. Failures are only
/// logged: the domains are stored and the reconciler converges routes on its
/// next pass.
pub async fn refresh_routes_logged(engine: &dyn Engine, service: &Service) {
    if let Err(e) = engine.refresh_routes(&service.id).await {
        tracing::warn!(service = %service.name, "refreshing routes failed (the reconciler will retry): {e}");
    }
}
