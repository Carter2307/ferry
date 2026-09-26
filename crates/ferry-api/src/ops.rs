//! Small side-effecting helpers shared by handlers and blueprints.

use ferry_core::{Datastore, DatastoreStatus, Deploy, DeployTrigger, Engine, Error, Result, Service, Store};

/// Restart a service so it picks up changed env vars. That's needed when it
/// has a live deploy, and also while a deploy is in flight (queued, building
/// or deploying): that deploy resolved — or will resolve — the env before the
/// change, so the engine queues the restart behind it. Suspended services and
/// services that were never deployed pick the new values up on their next
/// resume / deploy anyway. Returns the queued deploy, if any.
pub async fn restart_for_env_change(store: &Store, engine: &dyn Engine, service: &Service) -> Result<Option<Deploy>> {
    if service.suspended {
        return Ok(None);
    }
    if service.live_deploy_id.is_none() && !has_active_deploy(store, &service.id).await? {
        return Ok(None);
    }
    let deploy = engine.restart(&service.id, DeployTrigger::EnvChange).await?;
    tracing::info!(service = %service.name, deploy = %deploy.id, "restarting after env change");
    Ok(Some(deploy))
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

/// Restart every service linked to an env group (see [`restart_services`]).
pub async fn restart_group_services(store: &Store, engine: &dyn Engine, group_id: &str) -> Result<Vec<Deploy>> {
    let services = store.env_group_services(group_id).await?;
    Ok(restart_services(store, engine, &services).await)
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
