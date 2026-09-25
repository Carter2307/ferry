//! Small side-effecting helpers shared by handlers and blueprints.

use ferry_core::{Datastore, DatastoreStatus, Deploy, DeployTrigger, Engine, Error, Result, Service, Store};

/// Restart a service so it picks up changed env vars — only when it has a
/// live deploy and isn't suspended (otherwise the next deploy / resume uses
/// the new values anyway). Returns the queued deploy, if any.
pub async fn restart_for_env_change(engine: &dyn Engine, service: &Service) -> Result<Option<Deploy>> {
    if service.live_deploy_id.is_none() || service.suspended {
        return Ok(None);
    }
    let deploy = engine.restart(&service.id, DeployTrigger::EnvChange).await?;
    tracing::info!(service = %service.name, deploy = %deploy.id, "restarting after env change");
    Ok(Some(deploy))
}

/// Restart every live service linked to an env group, logging (not
/// returning) per-service failures. Returns the queued deploys.
pub async fn restart_group_services(store: &Store, engine: &dyn Engine, group_id: &str) -> Result<Vec<Deploy>> {
    let mut deploys = Vec::new();
    for svc in store.env_group_services(group_id).await? {
        match restart_for_env_change(engine, &svc).await {
            Ok(Some(d)) => deploys.push(d),
            Ok(None) => {}
            Err(e) => tracing::warn!(service = %svc.name, "restart after env group change failed: {e}"),
        }
    }
    Ok(deploys)
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
