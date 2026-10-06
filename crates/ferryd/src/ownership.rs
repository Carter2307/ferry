//! What a server owns: its data directory (a lock) and the Docker resources
//! of its name prefix (a marker volume). See DESIGN.md §15.

use std::path::Path;

use anyhow::Context;
use ferry_core::{Config, Store};

use crate::daemon;
use crate::fsutil::{read_private_file, write_private_file};

/// Hold an exclusive lock on `<data-dir>/ferryd.lock` for the process lifetime.
pub fn lock_data_dir(data_dir: &Path) -> anyhow::Result<std::fs::File> {
    let path = daemon::lock_path(data_dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("another ferryd is already running with data directory {}", data_dir.display())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {}", path.display())),
    }
}

/// Make sure the Docker resources of this name prefix belong to this data
/// directory, so two servers can never reconcile away each other's
/// containers. Ownership is recorded as labels on a marker volume
/// `<prefix>-owner`.
pub async fn claim_docker_prefix(
    docker: &ferry_docker::Docker,
    store: &Store,
    config: &Config,
    take_over: bool,
) -> anyhow::Result<()> {
    const OWNER: &str = "ferry.owner";
    const DATA_DIR: &str = "ferry.data-dir";
    let id_path = config.data_dir.join("instance_id");
    let owner_id = match read_private_file(&id_path) {
        Some(id) => id,
        None => {
            let id = ferry_core::ids::random_secret(24);
            write_private_file(&id_path, &id)?;
            id
        }
    };
    let prefix = &config.name_prefix;
    let marker = format!("{prefix}-owner");
    let naming = config.naming();
    let mut labels = naming.base_labels("owner");
    labels.insert(OWNER.to_string(), owner_id.clone());
    labels.insert(DATA_DIR.to_string(), config.data_dir.display().to_string());

    let existing = match docker.bollard().inspect_volume(&marker).await {
        Ok(v) => Some(v.labels),
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => None,
        Err(e) => return Err(anyhow::anyhow!("inspecting Docker volume {marker}: {e}")),
    };
    let conflict_hint = "Run this server with a different --name-prefix, stop the other server, or pass \
         --take-over to adopt the resources (the other server's containers will then be managed — and possibly \
         removed — by this one).";
    match existing {
        Some(l) if l.get(OWNER) == Some(&owner_id) => return Ok(()),
        Some(l) if !take_over => anyhow::bail!(
            "Docker resources with prefix '{prefix}' belong to another Ferry server (data dir {}). {conflict_hint}",
            l.get(DATA_DIR).map(String::as_str).unwrap_or("unknown")
        ),
        Some(_) => {
            tracing::warn!(prefix = %prefix, "taking over Docker resources owned by another Ferry data dir");
            docker.remove_volume(&marker).await.context("removing the old owner marker")?;
        }
        None => {
            // No marker: refuse to adopt containers of an unknown server when
            // this data dir is brand new (typical mistake: running ferryd from
            // another directory with the default ./ferry-data).
            let others = docker
                .list_containers(&[(ferry_core::naming::LABEL_INSTANCE, prefix.as_str())], true)
                .await
                .context("listing Docker containers")?;
            let fresh = store.list_services().await?.is_empty() && store.list_datastores().await?.is_empty();
            if !others.is_empty() && fresh && !take_over {
                anyhow::bail!(
                    "found {} Docker container(s) with prefix '{prefix}' that this (empty) data dir {} doesn't know. \
                     They probably belong to another Ferry server. {conflict_hint}",
                    others.len(),
                    config.data_dir.display()
                );
            }
        }
    }
    docker.ensure_volume(&marker, &labels).await.context("creating the owner marker volume")?;
    Ok(())
}
