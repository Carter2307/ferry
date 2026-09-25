//! Diff a [`Blueprint`] against the store: validation + the list of actions,
//! without writing anything.

use std::collections::{HashMap, HashSet};

use ferry_core::dto::BlueprintAction;
use ferry_core::{
    Config, Datastore, DatastoreKind, DatastoreStatus, EnvGroup, EnvVar, Error, Result, Runtime, Service, ServiceType,
    Store, git, ids, validate,
};

use super::{Blueprint, DatastoreSpec, EnvVarSpec, RefTarget, ServiceSpec};
use crate::checks;

/// Datastore properties understood by `ferry_core::env` (plus `hostport`,
/// compiled to `host:port`).
const DATASTORE_PROPS: &[&str] = &[
    "connectionString",
    "externalConnectionString",
    "host",
    "hostname",
    "port",
    "user",
    "username",
    "password",
    "database",
    "databaseName",
    "url",
    "internalUrl",
    "externalUrl",
    "hostport",
];

/// Service properties understood by `ferry_core::env`.
const SERVICE_PROPS: &[&str] = &["host", "hostname", "port", "hostport", "internalUrl", "url", "externalUrl"];

/// Length of `generateValue` secrets (hex characters → 256 bits).
const GENERATED_LEN: usize = 64;

/// The computed changes of a blueprint apply.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Every resource of the blueprint with its action, in apply order
    /// (env groups, datastores, services).
    pub actions: Vec<BlueprintAction>,
    /// Non-fatal problems found while planning (parse warnings not included).
    pub warnings: Vec<String>,
    pub(crate) groups: Vec<GroupPlan>,
    pub(crate) datastores: Vec<DatastorePlan>,
    pub(crate) services: Vec<ServicePlan>,
}

#[derive(Debug, Clone)]
pub(crate) struct GroupPlan {
    pub name: String,
    pub existing: Option<EnvGroup>,
    /// New or changed variables (upserted; nothing is deleted).
    pub set: Vec<EnvVar>,
}

#[derive(Debug, Clone)]
pub(crate) struct DatastorePlan {
    /// The row to insert (create) or the existing row.
    pub datastore: Datastore,
    pub create: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ServicePlan {
    /// Desired row. For updates it is based on the existing row.
    pub desired: Service,
    pub existing: Option<Service>,
    /// New or changed own variables.
    pub env_set: Vec<EnvVar>,
    /// Env groups (names) to link, in order; only links that don't exist yet.
    pub link_groups: Vec<String>,
    /// Stored settings differ from the existing row.
    pub settings_changed: bool,
    /// Build/deploy settings changed → the service needs a deploy.
    pub build_changed: bool,
    pub instances_changed: bool,
    pub domains_changed: bool,
    /// Own env vars or env group links changed → restart if live.
    pub env_changed: bool,
    /// Human-readable changes (updates only).
    pub changes: Vec<String>,
}

impl ServicePlan {
    pub fn has_source(&self) -> bool {
        self.desired.repo_url.is_some() || self.desired.image.is_some()
    }
}

/// A value slot while env vars are being computed.
#[derive(Debug, Clone, PartialEq)]
enum Slot {
    Ready(String),
    Copy { service: String, key: String },
    Skip,
}

/// Owner of a pending env var list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Group(usize),
    Service(usize),
}

struct Pending {
    owner: Owner,
    ctx: String,
    vars: Vec<(String, Slot)>,
    existing: Vec<EnvVar>,
}

/// What references may point at.
struct Targets<'a> {
    datastores: HashMap<&'a str, DatastoreKind>,
    services: HashSet<&'a str>,
}

fn prefix_err(ctx: &str, e: Error) -> Error {
    match e {
        Error::Invalid(m) => Error::Invalid(format!("{ctx}: {m}")),
        Error::Conflict(m) => Error::Conflict(format!("{ctx}: {m}")),
        other => other,
    }
}

/// Validate `bp` against the current state and compute the actions. Reads
/// the store; writes nothing.
pub async fn plan(store: &Store, config: &Config, bp: &Blueprint) -> Result<Plan> {
    let store_services = store.list_services().await?;
    let store_datastores = store.list_datastores().await?;
    let store_groups = store.list_env_groups().await?;
    let mut warnings: Vec<String> = Vec::new();

    // ---- names ---------------------------------------------------------------
    let mut group_names: HashSet<&str> = HashSet::new();
    for g in &bp.env_groups {
        validate::env_group_name(&g.name).map_err(|e| prefix_err(&format!("env group '{}'", g.name), e))?;
        if !group_names.insert(&g.name) {
            return Err(Error::invalid(format!("env group '{}' is declared more than once", g.name)));
        }
    }
    let mut resource_names: HashSet<&str> = HashSet::new();
    for d in &bp.datastores {
        let ctx = datastore_ctx(d);
        validate::resource_name(&d.name).map_err(|e| prefix_err(&ctx, e))?;
        if !resource_names.insert(&d.name) {
            return Err(Error::invalid(format!("name '{}' is used by more than one blueprint entry", d.name)));
        }
        if store_services.iter().any(|s| s.name == d.name) {
            return Err(Error::conflict(format!("{ctx}: the name is already used by a service")));
        }
    }
    for s in &bp.services {
        let ctx = format!("service '{}'", s.name);
        validate::resource_name(&s.name).map_err(|e| prefix_err(&ctx, e))?;
        if !resource_names.insert(&s.name) {
            return Err(Error::invalid(format!("name '{}' is used by more than one blueprint entry", s.name)));
        }
        if store_datastores.iter().any(|d| d.name == s.name) {
            return Err(Error::conflict(format!("{ctx}: the name is already used by a datastore")));
        }
    }

    let mut targets = Targets {
        datastores: store_datastores.iter().map(|d| (d.name.as_str(), d.kind)).collect(),
        services: store_services.iter().map(|s| s.name.as_str()).collect(),
    };
    for d in &bp.datastores {
        targets.datastores.insert(&d.name, d.kind);
    }
    for s in &bp.services {
        targets.services.insert(&s.name);
    }
    let known_groups: HashSet<&str> =
        store_groups.iter().map(|g| g.name.as_str()).chain(bp.env_groups.iter().map(|g| g.name.as_str())).collect();

    // ---- datastores ------------------------------------------------------------
    let mut datastores = Vec::new();
    let mut ds_actions = Vec::new();
    for spec in &bp.datastores {
        let (p, action) = plan_datastore(spec, &store_datastores, &mut warnings)?;
        datastores.push(p);
        ds_actions.push(action);
    }

    // ---- env var values (groups + services) ----------------------------------------
    let mut pending: Vec<Pending> = Vec::new();
    let mut group_existing: Vec<Option<EnvGroup>> = Vec::new();
    for (i, g) in bp.env_groups.iter().enumerate() {
        let existing = store_groups.iter().find(|x| x.name == g.name).cloned();
        let existing_vars = match &existing {
            Some(x) => store.list_env(&x.id).await?,
            None => Vec::new(),
        };
        let ctx = format!("env group '{}'", g.name);
        let hint = format!("ferry env-group set {}", g.name);
        let vars = initial_slots(&g.env_vars, &existing_vars, &ctx, &hint, &targets, &mut warnings)?;
        pending.push(Pending { owner: Owner::Group(i), ctx, vars, existing: existing_vars });
        group_existing.push(existing);
    }
    let mut service_existing: Vec<Option<Service>> = Vec::new();
    for (i, s) in bp.services.iter().enumerate() {
        let existing = store_services.iter().find(|x| x.name == s.name).cloned();
        let existing_vars = match &existing {
            Some(x) => store.list_env(&x.id).await?,
            None => Vec::new(),
        };
        let ctx = format!("service '{}'", s.name);
        for g in &s.env_groups {
            if !known_groups.contains(g.as_str()) {
                return Err(Error::invalid(format!("{ctx}: fromGroup references unknown env group '{g}'")));
            }
        }
        let hint = format!("ferry env set {}", s.name);
        let vars = initial_slots(&s.env_vars, &existing_vars, &ctx, &hint, &targets, &mut warnings)?;
        pending.push(Pending { owner: Owner::Service(i), ctx, vars, existing: existing_vars });
        service_existing.push(existing);
    }
    resolve_copies(store, &mut pending, bp, &store_services, &mut warnings).await?;

    // ---- env groups ------------------------------------------------------------
    let mut groups = Vec::new();
    let mut group_actions = Vec::new();
    for (i, g) in bp.env_groups.iter().enumerate() {
        let p = &pending[i];
        debug_assert_eq!(p.owner, Owner::Group(i));
        let (set, changes) = env_diff(&p.vars, &p.existing);
        let existing = group_existing[i].clone();
        let action = match (&existing, set.is_empty()) {
            (None, _) => "create",
            (Some(_), true) => "unchanged",
            (Some(_), false) => "update",
        };
        group_actions.push(BlueprintAction {
            resource: "env_group".into(),
            name: g.name.clone(),
            action: action.into(),
            changes: if existing.is_some() { changes } else { Vec::new() },
        });
        groups.push(GroupPlan { name: g.name.clone(), existing, set });
    }

    // ---- services ----------------------------------------------------------------
    let offset = bp.env_groups.len();
    let mut services: Vec<ServicePlan> = Vec::new();
    for (i, spec) in bp.services.iter().enumerate() {
        let ctx = format!("service '{}'", spec.name);
        let existing = service_existing[i].clone();
        let desired = desired_service(spec, existing.as_ref()).map_err(|e| prefix_err(&ctx, e))?;
        let p = &pending[offset + i];
        debug_assert_eq!(p.owner, Owner::Service(i));
        let (env_set, env_changes) = env_diff(&p.vars, &p.existing);
        let linked: Vec<String> = match &existing {
            Some(x) => store.service_env_groups(&x.id).await?.into_iter().map(|g| g.name).collect(),
            None => Vec::new(),
        };
        let link_groups: Vec<String> = spec.env_groups.iter().filter(|g| !linked.contains(g)).cloned().collect();

        let mut sp = ServicePlan {
            desired,
            existing: existing.clone(),
            env_changed: !env_set.is_empty() || !link_groups.is_empty(),
            env_set,
            link_groups,
            settings_changed: false,
            build_changed: false,
            instances_changed: false,
            domains_changed: false,
            changes: Vec::new(),
        };
        match &existing {
            Some(old) => {
                let mut changes = diff_service(old, &mut sp);
                changes.extend(env_changes);
                changes.extend(sp.link_groups.iter().map(|g| format!("env group {g}: linked")));
                sp.changes = changes;
                if sp.build_changed && !sp.has_source() {
                    warnings.push(format!(
                        "service '{0}' has no repo or image: redeploy it with `ferry up {0}` to apply the build changes",
                        spec.name
                    ));
                }
            }
            None => {
                if !sp.has_source() {
                    warnings
                        .push(format!("service '{0}' has no repo or image: deploy it with `ferry up {0}`", spec.name));
                }
            }
        }
        services.push(sp);
    }

    // ---- custom domains & default hosts -------------------------------------------
    let blueprint_names: HashSet<&str> = bp.services.iter().map(|s| s.name.as_str()).collect();
    let outside: Vec<&Service> = store_services.iter().filter(|s| !blueprint_names.contains(s.name.as_str())).collect();
    for (i, sp) in services.iter().enumerate() {
        let ctx = format!("service '{}'", sp.desired.name);
        let siblings: Vec<&Service> =
            services.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, p)| &p.desired).collect();
        // Clashes inside the blueprint are invalid input (400); clashes with
        // services outside it conflict with existing state (409).
        let inside = |e: Error| match e {
            Error::Conflict(m) => Error::Invalid(format!("{ctx}: {m}")),
            other => prefix_err(&ctx, other),
        };
        if sp.existing.is_none() || sp.domains_changed {
            checks::check_domains(config, siblings.iter().copied(), &sp.desired.name, &sp.desired.custom_domains)
                .map_err(inside)?;
            checks::check_domains(config, outside.iter().copied(), &sp.desired.name, &sp.desired.custom_domains)
                .map_err(|e| prefix_err(&ctx, e))?;
        }
        if sp.existing.is_none() {
            checks::check_default_host_free(config, siblings.iter().copied(), &sp.desired.name).map_err(inside)?;
            checks::check_default_host_free(config, outside.iter().copied(), &sp.desired.name)
                .map_err(|e| prefix_err(&ctx, e))?;
        }
    }

    // ---- actions ------------------------------------------------------------------
    let mut actions = group_actions;
    actions.extend(ds_actions);
    for sp in &services {
        let action = match (&sp.existing, sp.changes.is_empty()) {
            (None, _) => "create",
            (Some(_), true) => "unchanged",
            (Some(_), false) => "update",
        };
        actions.push(BlueprintAction {
            resource: "service".into(),
            name: sp.desired.name.clone(),
            action: action.into(),
            changes: sp.changes.clone(),
        });
    }

    Ok(Plan { actions, warnings, groups, datastores, services })
}

// ---------------------------------------------------------------------------
// datastores

fn datastore_ctx(d: &DatastoreSpec) -> String {
    match d.kind {
        DatastoreKind::Postgres => format!("database '{}'", d.name),
        DatastoreKind::Redis => format!("key value '{}'", d.name),
    }
}

fn plan_datastore(
    spec: &DatastoreSpec,
    store_datastores: &[Datastore],
    warnings: &mut Vec<String>,
) -> Result<(DatastorePlan, BlueprintAction)> {
    let ctx = datastore_ctx(spec);
    if let Some(v) = &spec.version {
        checks::datastore_version(v).map_err(|e| prefix_err(&ctx, e))?;
    }
    if let Some(db) = &spec.database {
        checks::pg_identifier("databaseName", db).map_err(|e| prefix_err(&ctx, e))?;
    }
    if let Some(u) = &spec.user {
        checks::pg_identifier("user", u).map_err(|e| prefix_err(&ctx, e))?;
    }
    let resource = "datastore".to_string();
    if let Some(existing) = store_datastores.iter().find(|d| d.name == spec.name) {
        if existing.kind != spec.kind {
            return Err(Error::conflict(format!(
                "{ctx}: an existing {} datastore already uses this name",
                existing.kind
            )));
        }
        let mut differs = |label: &str, want: &Option<String>, have: Option<&str>| {
            if let Some(w) = want
                && Some(w.as_str()) != have
            {
                warnings.push(format!(
                    "{ctx}: {label} '{w}' differs from the existing '{}'; datastores are not changed in place (ignored)",
                    have.unwrap_or("")
                ));
            }
        };
        differs("postgresMajorVersion", &spec.version, Some(existing.version.as_str()));
        differs("databaseName", &spec.database, existing.database.as_deref());
        differs("user", &spec.user, Some(existing.username.as_str()));
        if existing.status == DatastoreStatus::Failed {
            warnings.push(format!(
                "{ctx}: the existing datastore failed to provision: {}",
                existing.error.as_deref().unwrap_or("unknown error")
            ));
        }
        let action =
            BlueprintAction { resource, name: spec.name.clone(), action: "unchanged".into(), changes: Vec::new() };
        return Ok((DatastorePlan { datastore: existing.clone(), create: false }, action));
    }
    let mut ds = Datastore::new(spec.name.clone(), spec.kind);
    if let Some(v) = &spec.version {
        ds.version = v.clone();
    }
    if spec.kind == DatastoreKind::Postgres {
        if let Some(db) = &spec.database {
            ds.database = Some(db.clone());
            ds.username = db.clone();
        }
        if let Some(u) = &spec.user {
            ds.username = u.clone();
        }
    }
    let action = BlueprintAction { resource, name: spec.name.clone(), action: "create".into(), changes: Vec::new() };
    Ok((DatastorePlan { datastore: ds, create: true }, action))
}

// ---------------------------------------------------------------------------
// env vars

/// `${{kind.name.prop}}`.
pub(crate) fn reference(kind: &str, name: &str, prop: &str) -> String {
    format!("${{{{{kind}.{name}.{prop}}}}}")
}

fn compile_reference(
    target: RefTarget,
    name: &str,
    property: &str,
    ctx: &str,
    targets: &Targets<'_>,
) -> Result<String> {
    if let Some(kind) = targets.datastores.get(name) {
        if !DATASTORE_PROPS.contains(&property) {
            return Err(Error::invalid(format!(
                "{ctx}: unsupported property '{property}' for datastore '{name}' (expected connectionString, host, port, hostport, user, password or database)"
            )));
        }
        if *kind == DatastoreKind::Redis && matches!(property, "database" | "databaseName") {
            return Err(Error::invalid(format!("{ctx}: key value '{name}' has no database name")));
        }
        if property == "hostport" {
            return Ok(format!("{}:{}", reference("datastore", name, "host"), reference("datastore", name, "port")));
        }
        return Ok(reference("datastore", name, property));
    }
    if targets.services.contains(name) {
        if target == RefTarget::Datastore {
            return Err(Error::invalid(format!("{ctx}: '{name}' is a service, not a database or key value")));
        }
        if !SERVICE_PROPS.contains(&property) {
            return Err(Error::invalid(format!(
                "{ctx}: unsupported property '{property}' for service '{name}' (expected host, port, hostport, url or internalUrl)"
            )));
        }
        return Ok(reference("service", name, property));
    }
    let what = match target {
        RefTarget::Datastore => "database or key value",
        RefTarget::Service => "service",
        RefTarget::Any => "service or datastore",
    };
    Err(Error::invalid(format!("{ctx}: references unknown {what} '{name}'")))
}

fn initial_slots(
    specs: &[EnvVarSpec],
    existing: &[EnvVar],
    ctx: &str,
    set_hint: &str,
    targets: &Targets<'_>,
    warnings: &mut Vec<String>,
) -> Result<Vec<(String, Slot)>> {
    let mut out = Vec::new();
    for spec in specs {
        let key = spec.key().to_string();
        let kctx = format!("{ctx}: env var '{key}'");
        validate::env_key(&key).map_err(|e| prefix_err(ctx, e))?;
        let existing_value = existing.iter().find(|v| v.key == key).map(|v| v.value.clone());
        let slot = match spec {
            EnvVarSpec::Value { value, .. } => {
                if value.contains('\0') {
                    return Err(Error::invalid(format!("{kctx}: value contains a NUL byte")));
                }
                Slot::Ready(value.clone())
            }
            EnvVarSpec::Generate { .. } => {
                Slot::Ready(existing_value.unwrap_or_else(|| ids::random_secret(GENERATED_LEN)))
            }
            EnvVarSpec::Unsynced { .. } => {
                if existing_value.is_none() {
                    warnings.push(format!("{kctx} is marked sync: false; set it with `{set_hint} {key}=...`"));
                }
                Slot::Skip
            }
            EnvVarSpec::Reference { target, name, property, .. } => {
                Slot::Ready(compile_reference(*target, name, property, &kctx, targets)?)
            }
            EnvVarSpec::CopyFrom { service, env_var_key, .. } => {
                if !targets.services.contains(service.as_str()) {
                    return Err(Error::invalid(format!("{kctx}: fromService references unknown service '{service}'")));
                }
                validate::env_key(env_var_key).map_err(|e| prefix_err(&kctx, e))?;
                Slot::Copy { service: service.clone(), key: env_var_key.clone() }
            }
        };
        out.push((key, slot));
    }
    Ok(out)
}

/// Resolve `envVarKey` copies: from the planned values of blueprint services
/// first, then from the stored (effective) env of existing services.
async fn resolve_copies(
    store: &Store,
    pending: &mut [Pending],
    bp: &Blueprint,
    store_services: &[Service],
    warnings: &mut Vec<String>,
) -> Result<()> {
    // Preload the stored env of every copy target that exists.
    let mut stored: HashMap<String, Vec<EnvVar>> = HashMap::new();
    for p in pending.iter() {
        for (_, slot) in &p.vars {
            if let Slot::Copy { service, .. } = slot
                && !stored.contains_key(service)
                && let Some(s) = store_services.iter().find(|s| &s.name == service)
            {
                stored.insert(service.clone(), store.effective_env(&s.id).await?);
            }
        }
    }
    let service_index: HashMap<&str, usize> =
        bp.services.iter().enumerate().map(|(i, s)| (s.name.as_str(), i)).collect();

    loop {
        let mut updates: Vec<(usize, usize, Slot)> = Vec::new();
        for (pi, p) in pending.iter().enumerate() {
            for (vi, (_, slot)) in p.vars.iter().enumerate() {
                let Slot::Copy { service, key } = slot else { continue };
                let planned = service_index
                    .get(service.as_str())
                    .and_then(|i| pending.iter().find(|q| q.owner == Owner::Service(*i)))
                    .and_then(|q| q.vars.iter().find(|(k, _)| k == key))
                    .map(|(_, s)| s);
                let from_store = || stored.get(service).and_then(|vars| vars.iter().find(|v| &v.key == key));
                let resolved = match planned {
                    Some(Slot::Ready(v)) => Some(Slot::Ready(v.clone())),
                    Some(Slot::Copy { .. }) => None, // wait for the target to resolve
                    Some(Slot::Skip) | None => Some(match from_store() {
                        Some(v) => Slot::Ready(v.value.clone()),
                        None => {
                            warnings.push(format!(
                                "{}: env var '{}' not copied: service '{service}' has no variable '{key}' yet",
                                p.ctx, p.vars[vi].0
                            ));
                            Slot::Skip
                        }
                    }),
                };
                if let Some(s) = resolved {
                    updates.push((pi, vi, s));
                }
            }
        }
        if updates.is_empty() {
            break;
        }
        for (pi, vi, s) in updates {
            pending[pi].vars[vi].1 = s;
        }
    }
    // Whatever is left is a copy cycle.
    for p in pending.iter_mut() {
        for (k, slot) in p.vars.iter_mut() {
            if let Slot::Copy { service, key } = slot {
                warnings.push(format!(
                    "{}: env var '{k}' not copied: circular envVarKey reference through service '{service}' ({key})",
                    p.ctx
                ));
                *slot = Slot::Skip;
            }
        }
    }
    Ok(())
}

/// Variables to upsert and their human-readable changes.
fn env_diff(vars: &[(String, Slot)], existing: &[EnvVar]) -> (Vec<EnvVar>, Vec<String>) {
    let mut set = Vec::new();
    let mut changes = Vec::new();
    for (key, slot) in vars {
        let Slot::Ready(value) = slot else { continue };
        match existing.iter().find(|v| &v.key == key) {
            None => {
                changes.push(format!("env {key}: added"));
                set.push(EnvVar::new(key, value));
            }
            Some(v) if &v.value != value => {
                changes.push(format!("env {key}: updated"));
                set.push(EnvVar::new(key, value));
            }
            Some(_) => {}
        }
    }
    (set, changes)
}

// ---------------------------------------------------------------------------
// services

/// The desired row for `spec`: declarative for build/deploy settings (omitted
/// keys fall back to defaults), except that an existing service keeps its
/// current `numInstances` / `domains` when those are omitted, and keeps its
/// current source (repo + branch, or image) when neither `repo` nor `image`
/// is given — Render defaults `repo` to the blueprint's own repository, which
/// Ferry doesn't know.
fn desired_service(spec: &ServiceSpec, existing: Option<&Service>) -> Result<Service> {
    let mut s = match existing {
        Some(e) => e.clone(),
        None => Service::new(spec.name.clone(), spec.service_type),
    };
    s.service_type = spec.service_type;
    let keep_source = spec.repo_url.is_none() && spec.image.is_none();
    match existing {
        Some(e) if keep_source => s.branch = spec.branch.clone().unwrap_or_else(|| e.branch.clone()),
        _ => {
            s.repo_url = spec.repo_url.clone();
            s.image = spec.image.clone();
            s.branch = spec.branch.clone().unwrap_or_else(|| "main".to_string());
        }
    }
    s.runtime = spec.runtime.unwrap_or(if s.image.is_some() {
        Runtime::Image
    } else if spec.service_type == ServiceType::StaticSite {
        Runtime::Static
    } else {
        Runtime::Auto
    });
    s.root_dir = spec.root_dir.clone();
    s.dockerfile_path = spec.dockerfile_path.clone();
    s.build_command = spec.build_command.clone();
    s.start_command = spec.start_command.clone();
    s.publish_dir = spec.publish_dir.clone();
    s.port = spec.port;
    s.health_check_path = spec.health_check_path.clone();
    s.schedule = spec.schedule.clone();
    s.disk_mount_path = spec.disk_mount_path.clone();
    s.auto_deploy = spec.auto_deploy.unwrap_or(true);
    s.instances = spec.instances.or(existing.map(|e| e.instances)).unwrap_or(1);
    match &spec.domains {
        Some(d) => {
            let mut out = Vec::new();
            for x in d {
                out.push(validate::domain(x)?);
            }
            s.custom_domains = out;
        }
        None if existing.is_none() => s.custom_domains = Vec::new(),
        None => {}
    }
    validate::normalize_service(&mut s);
    validate::service(&s)?;
    Ok(s)
}

fn show(o: &Option<String>) -> String {
    o.clone().unwrap_or_else(|| "(none)".to_string())
}

fn show_list(l: &[String]) -> String {
    format!("[{}]", l.join(", "))
}

/// Compare the stored fields, set the change flags, return the changes.
fn diff_service(old: &Service, sp: &mut ServicePlan) -> Vec<String> {
    let new = &sp.desired;
    let mut changes = Vec::new();
    let mut build = false;
    let mut other = false;
    let mut field = |label: &str, a: String, b: String, is_build: bool| {
        if a != b {
            changes.push(format!("{label}: {a} → {b}"));
            if is_build {
                build = true;
            } else {
                other = true;
            }
        }
    };
    field("type", old.service_type.to_string(), new.service_type.to_string(), true);
    field(
        "repo_url",
        show(&old.repo_url.as_deref().map(git::redact_url)),
        show(&new.repo_url.as_deref().map(git::redact_url)),
        true,
    );
    field("branch", old.branch.clone(), new.branch.clone(), true);
    field("image", show(&old.image), show(&new.image), true);
    field("runtime", old.runtime.to_string(), new.runtime.to_string(), true);
    field("root_dir", show(&old.root_dir), show(&new.root_dir), true);
    field("dockerfile_path", show(&old.dockerfile_path), show(&new.dockerfile_path), true);
    field("build_command", show(&old.build_command), show(&new.build_command), true);
    field("start_command", show(&old.start_command), show(&new.start_command), true);
    field("publish_dir", show(&old.publish_dir), show(&new.publish_dir), true);
    field("port", show(&old.port.map(|p| p.to_string())), show(&new.port.map(|p| p.to_string())), true);
    field("health_check_path", show(&old.health_check_path), show(&new.health_check_path), true);
    field("disk_mount_path", show(&old.disk_mount_path), show(&new.disk_mount_path), true);
    field("schedule", show(&old.schedule), show(&new.schedule), false);
    field("auto_deploy", old.auto_deploy.to_string(), new.auto_deploy.to_string(), false);
    let instances_changed = old.instances != new.instances;
    let domains_changed = old.custom_domains != new.custom_domains;
    if instances_changed {
        changes.push(format!("instances: {} → {}", old.instances, new.instances));
    }
    if domains_changed {
        changes.push(format!(
            "custom_domains: {} → {}",
            show_list(&old.custom_domains),
            show_list(&new.custom_domains)
        ));
    }
    sp.build_changed = build;
    sp.instances_changed = instances_changed;
    sp.domains_changed = domains_changed;
    sp.settings_changed = build || other || instances_changed || domains_changed;
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_syntax() {
        assert_eq!(reference("datastore", "db", "connectionString"), "${{datastore.db.connectionString}}");
        let targets = Targets {
            datastores: [("db", DatastoreKind::Postgres), ("cache", DatastoreKind::Redis)].into_iter().collect(),
            services: ["api"].into_iter().collect(),
        };
        let c = |t, n, p| compile_reference(t, n, p, "x", &targets);
        assert_eq!(c(RefTarget::Datastore, "db", "password").unwrap(), "${{datastore.db.password}}");
        assert_eq!(
            c(RefTarget::Any, "cache", "hostport").unwrap(),
            "${{datastore.cache.host}}:${{datastore.cache.port}}"
        );
        assert_eq!(c(RefTarget::Service, "api", "hostport").unwrap(), "${{service.api.hostport}}");
        assert!(c(RefTarget::Datastore, "api", "host").is_err());
        assert!(c(RefTarget::Datastore, "cache", "database").is_err());
        assert!(c(RefTarget::Service, "api", "connectionString").is_err());
        assert!(c(RefTarget::Any, "nope", "host").is_err());
        assert!(c(RefTarget::Datastore, "db", "bogus").is_err());
    }

    #[test]
    fn env_diffs() {
        let vars = vec![
            ("A".to_string(), Slot::Ready("1".into())),
            ("B".to_string(), Slot::Ready("2".into())),
            ("C".to_string(), Slot::Skip),
        ];
        let (set, changes) = env_diff(&vars, &[EnvVar::new("A", "1"), EnvVar::new("B", "x"), EnvVar::new("C", "c")]);
        assert_eq!(set, vec![EnvVar::new("B", "2")]);
        assert_eq!(changes, vec!["env B: updated"]);
    }
}
