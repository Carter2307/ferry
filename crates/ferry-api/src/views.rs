//! Assembling API views (stored rows + computed fields).

use std::collections::HashMap;

use std::net::IpAddr;

use ferry_core::config::is_local_host;
use ferry_core::dto::{
    CertificateState, CertificateView, DatastoreView, DomainView, EnvGroupView, GitConnectionStatus, GitConnectionView,
    ServiceView,
};
use ferry_core::tls::{CertificateStatus, Certificates};
use ferry_core::{
    Config, Datastore, Deploy, DeploySource, Domain, EnvGroup, GitAuth, GitConnection, Result, Service, Store,
    compute_service_state, git, git_connection_for,
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

/// The names of the services each git connection clones for, by connection
/// id: services don't name a connection, their repository's URL decides
/// ([`git_connection_for`]).
pub async fn git_connection_users(
    store: &Store,
    connections: &[GitConnection],
) -> Result<HashMap<String, Vec<String>>> {
    let mut users: HashMap<String, Vec<String>> = HashMap::new();
    for service in store.list_services().await? {
        if let Some(repo) = service.repo_url.as_deref()
            && let Some(connection) = git_connection_for(connections, repo)
        {
            users.entry(connection.id.clone()).or_default().push(service.name);
        }
    }
    for names in users.values_mut() {
        names.sort();
    }
    Ok(users)
}

/// Build the [`GitConnectionView`] of a git connection: everything but its
/// secrets (of a personal access token, only a hint is shown).
pub fn git_connection_view(connection: GitConnection, services: Vec<String>) -> GitConnectionView {
    let status = if connection.is_connected() { GitConnectionStatus::Connected } else { GitConnectionStatus::Pending };
    // The tokens of the other kinds are renewed: when one expires says nothing.
    let personal = connection.auth == GitAuth::Token;
    let (app_slug, app_url) = match connection.app {
        Some(ref app) => (Some(app.slug.clone()), Some(app.url.clone())),
        None => (None, None),
    };
    let (manage_url, repository_selection) = match connection.installation {
        Some(ref installation) => (installation.url.clone(), installation.repository_selection.clone()),
        None => (None, None),
    };
    GitConnectionView {
        token_hint: connection.token_hint(),
        status,
        token_expires_at: connection.token_expires_at.filter(|_| personal),
        id: connection.id,
        provider: connection.provider,
        base_url: connection.base_url,
        auth: connection.auth,
        account: connection.account,
        account_name: connection.account_name,
        scopes: connection.scopes,
        client_id: connection.client_id,
        app_slug,
        app_url,
        manage_url,
        repository_selection,
        services,
        created_at: connection.created_at,
        updated_at: connection.updated_at,
    }
}

/// The view of one connection (with the services it clones for).
pub async fn git_connection_view_of(store: &Store, connection: GitConnection) -> Result<GitConnectionView> {
    let all = store.list_git_connections().await?;
    let services = git_connection_users(store, &all).await?.remove(&connection.id).unwrap_or_default();
    Ok(git_connection_view(connection, services))
}

/// Build the [`DomainView`] of a domain. `addresses` are the server's public
/// addresses, for the DNS records to create (none are needed for a local
/// domain).
pub fn domain_view(config: &Config, domain: Domain, addresses: &[IpAddr]) -> DomainView {
    let local = domain.is_local();
    DomainView {
        local,
        served: domain.is_served(),
        records: if local { Vec::new() } else { ferry_core::domains::dns_records(addresses) },
        url_pattern: config.url_for_host(&format!("<service>.{}", domain.name)),
        domain,
    }
}

/// Build the [`CertificateView`] of a hostname the proxy routes (`service`:
/// the name of its service, `None` for the dashboard). `certificates` is the
/// certificate manager, `None` when the server runs without HTTPS.
pub fn certificate_view(
    certificates: Option<&dyn Certificates>,
    host: String,
    service: Option<String>,
) -> CertificateView {
    let mut view = CertificateView {
        state: CertificateState::Disabled,
        expires_at: None,
        error: None,
        retry_at: None,
        host,
        service,
    };
    if is_local_host(&view.host) {
        view.state = CertificateState::Local;
        return view;
    }
    let Some(certificates) = certificates else { return view };
    match certificates.certificate(&view.host) {
        CertificateStatus::Issued { not_after } => {
            view.state = CertificateState::Issued;
            view.expires_at = Some(not_after);
        }
        CertificateStatus::Issuing => view.state = CertificateState::Issuing,
        CertificateStatus::Pending => view.state = CertificateState::Pending,
        CertificateStatus::Failed { error, retry_at } => {
            view.state = CertificateState::Failed;
            view.error = Some(error);
            view.retry_at = retry_at;
        }
    }
    view
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
