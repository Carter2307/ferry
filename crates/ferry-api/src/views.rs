//! Assembling API views (stored rows + computed fields).

use ferry_core::dto::{DatastoreView, EnvGroupView, ServiceView};
use ferry_core::{Config, Datastore, EnvGroup, Result, Service, Store, compute_service_state};

/// Path (with secret key) that triggers a deploy without the API token.
pub fn deploy_hook_path(service: &Service) -> String {
    format!("/hooks/deploy/{}?key={}", service.id, service.deploy_hook_key)
}

/// Build the [`ServiceView`] of a service.
pub async fn service_view(store: &Store, config: &Config, service: Service) -> Result<ServiceView> {
    let latest = store.latest_deploy(&service.id).await?;
    let state = compute_service_state(&service, latest.as_ref(), None);
    let mut internal_port = service.port;
    if internal_port.is_none()
        && let Some(live_id) = &service.live_deploy_id
    {
        internal_port = match &latest {
            Some(d) if &d.id == live_id => d.port,
            _ => store.get_deploy(live_id).await?.and_then(|d| d.port),
        };
    }
    let env_groups = store.service_env_groups(&service.id).await?.into_iter().map(|g| g.name).collect();
    Ok(ServiceView {
        state,
        url: config.service_url(&service),
        hosts: config.service_hosts(&service),
        internal_host: service.name.clone(),
        internal_port,
        env_groups,
        latest_deploy: latest,
        deploy_hook_path: deploy_hook_path(&service),
        service,
    })
}

/// Build the [`DatastoreView`] of a datastore.
pub fn datastore_view(config: &Config, datastore: Datastore) -> DatastoreView {
    DatastoreView {
        internal_host: datastore.internal_host().to_string(),
        internal_port: datastore.internal_port(),
        internal_url: datastore.internal_url(),
        external_url: datastore.external_url(&config.advertise_host),
        datastore,
    }
}

/// Build the [`EnvGroupView`] of an env group.
pub async fn env_group_view(store: &Store, group: EnvGroup) -> Result<EnvGroupView> {
    let vars = store.list_env(&group.id).await?;
    let services = store.env_group_services(&group.id).await?.into_iter().map(|s| s.name).collect();
    Ok(EnvGroupView { group, vars, services })
}
