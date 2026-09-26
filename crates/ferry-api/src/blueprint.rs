//! Blueprints: declarative `ferry.yaml` / `render.yaml` files (DESIGN.md §Blueprints).
//!
//! `parse` turns YAML into a typed [`Blueprint`]; `plan`/`apply` diff it
//! against the store and create/update env groups, datastores and services
//! (never deleting anything), then queue deploys for new or changed services.
//!
//! * [`parse`] is lenient about the schema: unknown or unsupported keys
//!   (`plan`, `region`, `scaling`, `previews`, …) become warnings, never
//!   errors, so real Render `render.yaml` files are accepted. Structural
//!   problems (missing `name`, unknown service `type`, malformed env vars)
//!   are errors naming the entry.
//! * [`plan`] validates every entry against the current state (names,
//!   service settings, custom domains, env var references) and computes the
//!   `create` / `update` / `unchanged` actions without writing anything.
//! * [`apply`] runs the plan (unless `dry_run`): env groups → datastores
//!   (create + provision) → services, then queues `blueprint` deploys for new
//!   services with a source and for services whose build/deploy settings
//!   changed, and restarts live services whose only changes are env vars.

mod apply;
mod parse;
mod plan;

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
