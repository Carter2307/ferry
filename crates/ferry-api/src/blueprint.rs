//! Blueprints: declarative `ferry.yaml` / `render.yaml` files (DESIGN.md §Blueprints).
//!
//! `parse` turns YAML into a typed [`Blueprint`]; `plan`/`apply` diff it
//! against the store and create/update env groups, datastores and services
//! (never deleting anything), then queue deploys for new or changed services.
//!
//! * [`parse`] is lenient about the schema: unknown or unsupported keys
//!   (`region`, `scaling`, `previews`, …) become warnings, never errors, so
//!   real Render `render.yaml` files are accepted. Structural problems
//!   (missing `name`, unknown service `type`, malformed env vars) are errors
//!   naming the entry. Render's `plan` becomes resource limits ([`plans`]),
//!   overridden by the Ferry keys `memoryLimit` / `cpuLimit`.
//! * [`plan`] validates every entry against the current state (names,
//!   service settings, custom domains, env var references) and computes the
//!   `create` / `update` / `unchanged` actions without writing anything.
//! * [`apply`] runs the plan (unless `dry_run`): env groups → datastores
//!   (create + provision) → services, then queues `blueprint` deploys for new
//!   services with a source and for services whose build/deploy settings
//!   changed, restarts live services whose only changes are env vars or
//!   resource limits, and applies datastores' new limits in place.

mod apply;
mod parse;
mod plan;
pub mod plans;

use ferry_core::{DatastoreKind, Runtime, ServiceType};

pub use apply::apply;
pub use parse::parse;
pub use plan::{Plan, plan};

/// A parsed blueprint.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Blueprint {
    /// `envVarGroups`.
    pub env_groups: Vec<EnvGroupSpec>,
    /// `databases` (Postgres) followed by `redis` / `keyvalue` services.
    pub datastores: Vec<DatastoreSpec>,
    /// Every other entry of `services`.
    pub services: Vec<ServiceSpec>,
    /// Non-fatal parse problems (ignored keys, unsupported runtimes, …).
    pub warnings: Vec<String>,
}

/// An entry of `envVarGroups`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvGroupSpec {
    pub name: String,
    pub env_vars: Vec<EnvVarSpec>,
}

/// A Postgres database (`databases`) or Redis instance (`type: redis|keyvalue`).
#[derive(Debug, Clone, PartialEq)]
pub struct DatastoreSpec {
    pub name: String,
    pub kind: DatastoreKind,
    /// `postgresMajorVersion`.
    pub version: Option<String>,
    /// `databaseName`.
    pub database: Option<String>,
    /// `user`.
    pub user: Option<String>,
    /// `plan`, `memoryLimit`, `cpuLimit`.
    pub limits: LimitsSpec,
}

/// Resource limits declared by a blueprint entry: its `plan` (see
/// [`plans`]), overridden key by key by the Ferry extension keys
/// `memoryLimit` (a size like `1G`, or a number of MiB) and `cpuLimit` (CPUs,
/// or millicores like `500m`).
///
/// `None` = not declared: a new resource gets the server default, and an
/// existing one keeps its current limit (like `numInstances`: limits are
/// often tuned outside the blueprint, and re-applying one without a `plan`
/// must not silently reset them). `Some(0)` / `Some(0.0)` (`memoryLimit: 0`)
/// = explicitly the server default, like `0` in the API.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LimitsSpec {
    /// Memory limit, MiB.
    pub memory_mb: Option<u32>,
    /// CPU limit, CPUs.
    pub cpus: Option<f64>,
}

impl LimitsSpec {
    /// The limits after applying this spec to `current` (`None` = the server
    /// default).
    pub fn resolve(&self, current: (Option<u32>, Option<f64>)) -> (Option<u32>, Option<f64>) {
        let memory = match self.memory_mb {
            Some(m) => (m != 0).then_some(m),
            None => current.0,
        };
        let cpus = match self.cpus {
            Some(c) => (c != 0.0).then_some(c),
            None => current.1,
        };
        (memory, cpus)
    }
}

/// A service entry (everything but `redis` / `keyvalue`).
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceSpec {
    pub name: String,
    pub service_type: ServiceType,
    /// `runtime` (or the legacy `env`); `None` = image when `image` is set,
    /// static for static sites, else auto-detect.
    pub runtime: Option<Runtime>,
    /// `repo`.
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    /// `image.url` (or a plain string).
    pub image: Option<String>,
    /// `rootDir` (or `dockerContext` when no `rootDir` is given).
    pub root_dir: Option<String>,
    /// `dockerfilePath`, relative to `root_dir`.
    pub dockerfile_path: Option<String>,
    /// `buildCommand`.
    pub build_command: Option<String>,
    /// `startCommand` (or `dockerCommand`).
    pub start_command: Option<String>,
    /// `staticPublishPath`.
    pub publish_dir: Option<String>,
    /// `port` (Ferry extension).
    pub port: Option<u16>,
    /// `healthCheckPath`.
    pub health_check_path: Option<String>,
    /// `numInstances` (`None` keeps the current count of an existing service).
    pub instances: Option<u32>,
    /// `autoDeploy` / `autoDeployTrigger` (default true).
    pub auto_deploy: Option<bool>,
    pub schedule: Option<String>,
    /// `disk.mountPath`.
    pub disk_mount_path: Option<String>,
    /// `domains` (`None` keeps the current custom domains of an existing service).
    pub domains: Option<Vec<String>>,
    /// `plan`, `memoryLimit`, `cpuLimit` (undeclared limits keep the current
    /// ones of an existing service).
    pub limits: LimitsSpec,
    pub env_vars: Vec<EnvVarSpec>,
    /// `fromGroup` entries, in order.
    pub env_groups: Vec<String>,
}

impl ServiceSpec {
    /// A spec with only a name and a type (everything else unset).
    pub fn new(name: impl Into<String>, service_type: ServiceType) -> Self {
        ServiceSpec {
            name: name.into(),
            service_type,
            runtime: None,
            repo_url: None,
            branch: None,
            image: None,
            root_dir: None,
            dockerfile_path: None,
            build_command: None,
            start_command: None,
            publish_dir: None,
            port: None,
            health_check_path: None,
            instances: None,
            auto_deploy: None,
            schedule: None,
            disk_mount_path: None,
            domains: None,
            limits: LimitsSpec::default(),
            env_vars: Vec::new(),
            env_groups: Vec::new(),
        }
    }
}

/// What a reference (`fromDatabase` / `fromService … property`) points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefTarget {
    /// `fromDatabase`: a Postgres database.
    Database,
    /// `fromService` with `type: redis|keyvalue`: a Key Value (Redis) instance.
    KeyValue,
    /// `fromService` with a service type.
    Service,
    /// `fromService` without a `type`: resolved by name.
    Any,
}

impl RefTarget {
    /// A datastore reference (`fromDatabase`, or `fromService` of a key value).
    pub fn is_datastore(self) -> bool {
        matches!(self, RefTarget::Database | RefTarget::KeyValue)
    }
}

/// One entry of `envVars`.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvVarSpec {
    /// `{key, value}`.
    Value { key: String, value: String },
    /// `{key, generateValue: true}`: a random secret, generated only when the
    /// variable doesn't exist yet (never rotated by a re-apply).
    Generate { key: String },
    /// `{key, sync: false}`: left for the user to set (warning).
    Unsynced { key: String },
    /// `fromDatabase` / `fromService` with a `property`: compiled to a
    /// `${{datastore.NAME.PROP}}` / `${{service.NAME.PROP}}` reference.
    Reference { key: String, target: RefTarget, name: String, property: String },
    /// `fromService` with `envVarKey`: the value of another service's
    /// variable, copied at apply time.
    CopyFrom { key: String, service: String, env_var_key: String },
}

impl EnvVarSpec {
    pub fn key(&self) -> &str {
        match self {
            EnvVarSpec::Value { key, .. }
            | EnvVarSpec::Generate { key }
            | EnvVarSpec::Unsynced { key }
            | EnvVarSpec::Reference { key, .. }
            | EnvVarSpec::CopyFrom { key, .. } => key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_specs_resolve_against_the_current_limits() {
        let spec = |memory_mb, cpus| LimitsSpec { memory_mb, cpus };
        let current = (Some(1024), Some(2.0));
        // not declared: keep
        assert_eq!(spec(None, None).resolve(current), current);
        assert_eq!(spec(None, None).resolve((None, None)), (None, None));
        // declared: replace, key by key
        assert_eq!(spec(Some(512), None).resolve(current), (Some(512), Some(2.0)));
        assert_eq!(spec(None, Some(0.5)).resolve(current), (Some(1024), Some(0.5)));
        // 0: back to the server default
        assert_eq!(spec(Some(0), Some(0.0)).resolve(current), (None, None));
        assert_eq!(spec(Some(0), Some(1.0)).resolve((None, None)), (None, Some(1.0)));
    }
}
