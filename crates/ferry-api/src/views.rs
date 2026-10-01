//! Assembling API views (stored rows + computed fields).

use ferry_core::dto::{DatastoreView, EnvGroupView, GitConnectionView, ServiceView};
use ferry_core::{
    Config, Datastore, Deploy, DeploySource, EnvGroup, GitConnection, Result, Service, Store, compute_service_state,
    git,
};

use crate::runtime;

/// Path (with secret key) that triggers a deploy without the API token.
pub fn deploy_hook_path(service: &Service) -> String {
    format!("/hooks/deploy/{}?key={}", service.id, service.deploy_hook_key)
}

/// Build the [`ServiceView`] of a service. Its state uses the running
/// instances observed recently ([`runtime`]), if any; its `internal_port` is
/// the live deploy's port, else the configured one.
pub async fn service_view(store: &Store, config: &Config, service: Service) -> Result<ServiceView> {
    let latest = store.latest_deploy(&service.id).await?;
    let state = compute_service_state(&service, latest.as_ref(), runtime::running(&service));
    // The port the live instances actually listen on wins over the setting:
    // a changed `port` only applies from the next deploy.
    let live_port = match &service.live_deploy_id {
        Some(live_id) => match &latest {
            Some(d) if &d.id == live_id => d.port,
            _ => store.get_deploy(live_id).await?.and_then(|d| d.port),
        },
        None => None,
    };
    let internal_port = live_port.or(service.port);
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

/// A deploy as returned to unauthenticated callers (webhooks): credentials
/// embedded in the repository URL are redacted and the server-side path of
/// an uploaded archive is reduced to its file name.
pub fn public_deploy(mut deploy: Deploy) -> Deploy {
    match &mut deploy.source {
        DeploySource::Git { repo_url, .. } => *repo_url = git::redact_url(repo_url),
        DeploySource::Archive { path } => {
            let name = std::path::Path::new(path.as_str()).file_name().map(|n| n.to_string_lossy().into_owned());
            *path = name.unwrap_or_default();
        }
        DeploySource::Image { .. } | DeploySource::Reuse { .. } => {}
    }
    deploy
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

/// Build the [`GitConnectionView`] of a git connection: everything but the
/// token, of which only a hint is shown.
pub async fn git_connection_view(store: &Store, connection: GitConnection) -> Result<GitConnectionView> {
    let services = store.git_connection_services(&connection.id).await?.into_iter().map(|s| s.name).collect();
    Ok(GitConnectionView {
        token_hint: connection.token_hint(),
        id: connection.id,
        provider: connection.provider,
        base_url: connection.base_url,
        account: connection.account,
        account_name: connection.account_name,
        scopes: connection.scopes,
        token_expires_at: connection.token_expires_at,
        services,
        created_at: connection.created_at,
        updated_at: connection.updated_at,
    })
}

/// Build the [`EnvGroupView`] of an env group.
pub async fn env_group_view(store: &Store, group: EnvGroup) -> Result<EnvGroupView> {
    let vars = store.list_env(&group.id).await?;
    let services = store.env_group_services(&group.id).await?.into_iter().map(|s| s.name).collect();
    Ok(EnvGroupView { group, vars, services })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::DeployTrigger;

    #[test]
    fn public_deploys_hide_credentials_and_paths() {
        let git = DeploySource::Git {
            repo_url: "https://user:s3cret@github.com/a/b".into(),
            branch: "main".into(),
            commit: None,
        };
        let d = public_deploy(Deploy::new("srv-x", DeployTrigger::DeployHook, git));
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
        assert!(json.contains("https://***@github.com/a/b"), "{json}");
        let archive = DeploySource::Archive { path: "/var/lib/ferry/uploads/upl-1.tar.gz".into() };
        let d = public_deploy(Deploy::new("srv-x", DeployTrigger::DeployHook, archive));
        assert_eq!(d.source, DeploySource::Archive { path: "upl-1.tar.gz".into() });
    }
}
