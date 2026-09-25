//! Execute a [`Plan`](super::Plan).

use std::collections::{HashMap, HashSet};

use ferry_core::dto::BlueprintResult;
use ferry_core::{Config, DeployRequest, DeployTrigger, Engine, EnvGroup, Error, Result, Store};

use super::{Blueprint, plan};
use crate::ops;

/// Validate and apply a blueprint. With `dry_run`, only the plan is computed
/// and nothing is written. Every entry is validated before the first write;
/// failures of engine side effects after the writes (deploys, restarts,
/// provisioning) are reported as warnings.
pub async fn apply(
    store: &Store,
    config: &Config,
    engine: &dyn Engine,
    bp: &Blueprint,
    dry_run: bool,
) -> Result<BlueprintResult> {
    let plan = plan(store, config, bp).await?;
    let mut warnings = bp.warnings.clone();
    warnings.extend(plan.warnings.iter().cloned());
    if dry_run {
        return Ok(BlueprintResult { dry_run: true, actions: plan.actions, deploys: Vec::new(), warnings });
    }
    let mut deploys = Vec::new();

    // 1. env groups
    let mut group_ids: HashMap<String, String> =
        store.list_env_groups().await?.into_iter().map(|g| (g.name, g.id)).collect();
    let mut changed_groups: Vec<String> = Vec::new();
    for gp in &plan.groups {
        let id = match &gp.existing {
            Some(g) => g.id.clone(),
            None => {
                let g = EnvGroup::new(gp.name.clone());
                store.create_env_group(&g).await?;
                tracing::info!(group = %g.name, "blueprint: created env group");
                g.id
            }
        };
        if !gp.set.is_empty() {
            store.patch_env(&id, &gp.set, &[]).await?;
            if gp.existing.is_some() {
                store.touch_env_group(&id).await?;
                changed_groups.push(id.clone());
            }
        }
        group_ids.insert(gp.name.clone(), id);
    }

    // 2. datastores (create + provision)
    for dp in &plan.datastores {
        if !dp.create {
            continue;
        }
        let ds = &dp.datastore;
        store.create_datastore(ds).await?;
        tracing::info!(datastore = %ds.name, kind = %ds.kind, "blueprint: created datastore");
        if let Err(e) = engine.provision_datastore(&ds.id).await {
            warnings.push(format!("datastore '{}': provisioning failed: {e}", ds.name));
            ops::mark_datastore_failed(store, &ds.id, &e).await;
        }
    }

    // 3. services (rows, env vars, env group links)
    let mut ids: Vec<String> = Vec::with_capacity(plan.services.len());
    for sp in &plan.services {
        let id = match &sp.existing {
            None => {
                store.create_service(&sp.desired).await?;
                tracing::info!(service = %sp.desired.name, "blueprint: created service");
                sp.desired.id.clone()
            }
            Some(old) => {
                if sp.settings_changed {
                    // Runtime-managed fields come from the freshest row.
                    let fresh = store.require_service(&old.id).await?;
                    let mut row = sp.desired.clone();
                    row.instances = fresh.instances;
                    row.suspended = fresh.suspended;
                    row.live_deploy_id = fresh.live_deploy_id;
                    row.deploy_hook_key = fresh.deploy_hook_key;
                    store.update_service(&row).await?;
                    tracing::info!(service = %row.name, "blueprint: updated service");
                }
                old.id.clone()
            }
        };
        if !sp.env_set.is_empty() {
            store.patch_env(&id, &sp.env_set, &[]).await?;
        }
        for g in &sp.link_groups {
            let gid = group_ids
                .get(g)
                .ok_or_else(|| Error::internal(format!("env group '{g}' disappeared during the blueprint apply")))?;
            store.link_env_group(&id, gid).await?;
        }
        ids.push(id);
    }

    // 4. deploys, restarts, scaling, routes
    let mut group_members: HashSet<String> = HashSet::new();
    for g in &changed_groups {
        group_members.extend(store.env_group_services(g).await?.into_iter().map(|s| s.id));
    }
    let mut handled: HashSet<String> = HashSet::new();
    for (sp, id) in plan.services.iter().zip(&ids) {
        handled.insert(id.clone());
        let name = &sp.desired.name;
        if sp.existing.is_none() {
            if sp.has_source() {
                match engine.deploy(id, DeployRequest::new(DeployTrigger::Blueprint)).await {
                    Ok(d) => deploys.push(d),
                    Err(e) => warnings.push(format!("service '{name}': queuing the first deploy failed: {e}")),
                }
            }
            continue;
        }
        if sp.instances_changed
            && let Err(e) = engine.scale(id, sp.desired.instances).await
        {
            warnings.push(format!("service '{name}': scaling to {} failed: {e}", sp.desired.instances));
        }
        let current = store.require_service(id).await?;
        if sp.domains_changed {
            ops::refresh_routes_logged(engine, &current).await;
        }
        if sp.build_changed && sp.has_source() {
            if current.suspended {
                warnings.push(format!("service '{name}' is suspended: resume it and deploy to apply the changes"));
                continue;
            }
            match engine.deploy(id, DeployRequest::new(DeployTrigger::Blueprint)).await {
                Ok(d) => deploys.push(d),
                Err(e) => warnings.push(format!("service '{name}': queuing a deploy failed: {e}")),
            }
        } else if sp.env_changed || group_members.contains(id) {
            match ops::restart_for_env_change(engine, &current).await {
                Ok(Some(d)) => deploys.push(d),
                Ok(None) => {}
                Err(e) => warnings.push(format!("service '{name}': restart after env change failed: {e}")),
            }
        }
    }
    // Services outside the blueprint that use a changed env group.
    let mut outside: Vec<&String> = group_members.difference(&handled).collect();
    outside.sort();
    for id in outside {
        let Some(svc) = store.get_service(id).await? else { continue };
        match ops::restart_for_env_change(engine, &svc).await {
            Ok(Some(d)) => deploys.push(d),
            Ok(None) => {}
            Err(e) => warnings.push(format!("service '{}': restart after env group change failed: {e}", svc.name)),
        }
    }

    Ok(BlueprintResult { dry_run: false, actions: plan.actions, deploys, warnings })
}
