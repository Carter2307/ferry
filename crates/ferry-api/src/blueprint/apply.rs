//! Execute a [`Plan`](super::Plan).

use std::collections::{HashMap, HashSet};

use ferry_core::dto::BlueprintResult;
use ferry_core::{Config, Datastore, DeployRequest, DeployTrigger, Engine, EnvGroup, Error, Result, Store};

use super::{Blueprint, plan};
use crate::{locks, ops};

/// Validate and apply a blueprint. With `dry_run`, only the plan is computed
/// and nothing is written. Every entry is validated before the first write;
/// failures of engine side effects after the writes (deploys, restarts,
/// provisioning) are reported as warnings.
///
/// Planning and writing run under the names lock, the domain lock and the
/// row locks of every existing resource the blueprint names (services, env
/// groups, datastores; see [`crate::locks`]), so API requests can't create a
/// clashing service or datastore, or change those resources, between the
/// checks and the writes. Callers must not hold any of those locks.
pub async fn apply(
    store: &Store,
    config: &Config,
    engine: &dyn Engine,
    bp: &Blueprint,
    dry_run: bool,
) -> Result<BlueprintResult> {
    if dry_run {
        let plan = plan(store, config, bp).await?;
        let mut warnings = bp.warnings.clone();
        warnings.extend(plan.warnings.iter().cloned());
        return Ok(BlueprintResult { dry_run: true, actions: plan.actions, deploys: Vec::new(), warnings });
    }

    let names_guard = locks::names().await;
    let domains_guard = locks::domains().await;
    let service_names: HashSet<&str> = bp.services.iter().map(|s| s.name.as_str()).collect();
    let group_names: HashSet<&str> = bp.env_groups.iter().map(|g| g.name.as_str()).collect();
    let datastore_names: HashSet<&str> = bp.datastores.iter().map(|d| d.name.as_str()).collect();
    let mut owners: Vec<String> = store
        .list_services()
        .await?
        .into_iter()
        .filter(|s| service_names.contains(s.name.as_str()))
        .map(|s| s.id)
        .collect();
    owners.extend(
        store.list_env_groups().await?.into_iter().filter(|g| group_names.contains(g.name.as_str())).map(|g| g.id),
    );
    owners.extend(
        store.list_datastores().await?.into_iter().filter(|d| datastore_names.contains(d.name.as_str())).map(|d| d.id),
    );
    let row_guards = locks::owners(owners).await;

    let plan = plan(store, config, bp).await?;
    let mut warnings = bp.warnings.clone();
    warnings.extend(plan.warnings.iter().cloned());
    let mut deploys = Vec::new();

    // The environments the restarts below compare against: only services
    // whose effective environment really changes are restarted.
    let mut env_candidates: Vec<String> =
        plan.services.iter().filter_map(|sp| sp.existing.as_ref()).map(|s| s.id.clone()).collect();
    for gp in plan.groups.iter().filter(|gp| !gp.set.is_empty()) {
        if let Some(g) = &gp.existing {
            env_candidates.extend(store.env_group_services(&g.id).await?.into_iter().map(|s| s.id));
        }
    }
    let env_before = ops::EnvSnapshot::capture(store, &env_candidates).await?;

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

    // 2. datastores (create + provision; new limits of existing ones)
    let mut resized: Vec<&Datastore> = Vec::new();
    for dp in &plan.datastores {
        let ds = &dp.datastore;
        if !dp.create {
            if dp.limits_changed {
                store.set_datastore_limits(&ds.id, ds.memory_limit_mb, ds.cpu_limit).await?;
                tracing::info!(datastore = %ds.name, "blueprint: changed datastore limits");
                // Applied to its container (below, once the locks are
                // released) whatever its status: a failed datastore's
                // container may still be there, and provisioning keeps it.
                resized.push(ds);
            }
            continue;
        }
        store.create_datastore(ds).await?;
        tracing::info!(datastore = %ds.name, kind = %ds.kind, "blueprint: created datastore");
        if let Err(e) = engine.provision_datastore(&ds.id).await {
            warnings.push(format!("datastore '{}': provisioning failed: {e}", ds.name));
            ops::mark_datastore_failed(store, &ds.id, &e).await;
        }
    }

    // 3. services (rows, env vars, env group links)
    let group_id = |g: &String| {
        group_ids
            .get(g)
            .cloned()
            .ok_or_else(|| Error::internal(format!("env group '{g}' disappeared during the blueprint apply")))
    };
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
                    // Engine-owned columns (instances, suspended, live deploy)
                    // are never written by `update_service`; the hook key comes
                    // from the stored row.
                    let fresh = store.require_service(&old.id).await?;
                    let mut row = sp.desired.clone();
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
        for g in &sp.unlink_groups {
            store.unlink_env_group(&id, &group_id(g)?).await?;
        }
        for g in &sp.link_groups {
            store.link_env_group(&id, &group_id(g)?).await?;
        }
        ids.push(id);
    }
    // The rows are written: API requests may proceed while the engine works.
    drop(row_guards);
    drop(domains_guard);
    drop(names_guard);

    // 4. datastore limits, deploys, restarts, scaling, routes
    for ds in resized {
        if let Err(e) = engine.update_datastore_limits(&ds.id).await {
            warnings
                .push(format!("datastore '{}': limits saved, but applying them to its container failed: {e}", ds.name));
        }
    }
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
        if (sp.build_changed || sp.needs_deploy) && sp.has_source() {
            if current.suspended {
                warnings.push(format!("service '{name}' is suspended: resume it and deploy to apply the changes"));
                continue;
            }
            match engine.deploy(id, DeployRequest::new(DeployTrigger::Blueprint)).await {
                Ok(d) => deploys.push(d),
                Err(e) => warnings.push(format!("service '{name}': queuing a deploy failed: {e}")),
            }
        } else if sp.command_changed || sp.limits_changed {
            // Both only apply through a deploy's launch spec; the restart
            // also applies env changes, if any.
            let what = match (sp.command_changed, sp.limits_changed) {
                (true, true) => "command and resource limits",
                (true, false) => "command",
                _ => "resource limits",
            };
            if current.suspended && current.live_deploy_id.is_some() {
                warnings.push(format!("service '{name}' is suspended: resume and restart it to apply the new {what}"));
                continue;
            }
            match ops::restart_if_deployed(store, engine, &current, DeployTrigger::Restart).await {
                Ok(Some(d)) => deploys.push(d),
                Ok(None) => {}
                Err(e) => warnings.push(format!("service '{name}': restart for the new {what} failed: {e}")),
            }
        } else if (sp.env_changed || group_members.contains(id)) && env_before.changed(store, id).await.unwrap_or(true)
        {
            match ops::restart_for_env_change(store, engine, &current).await {
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
        if !env_before.changed(store, id).await.unwrap_or(true) {
            continue;
        }
        match ops::restart_for_env_change(store, engine, &svc).await {
            Ok(Some(d)) => deploys.push(d),
            Ok(None) => {}
            Err(e) => warnings.push(format!("service '{}': restart after env group change failed: {e}", svc.name)),
        }
    }

    Ok(BlueprintResult { dry_run: false, actions: plan.actions, deploys, warnings })
}
