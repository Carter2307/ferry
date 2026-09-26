//! Command-line syntax (clap derive). See DESIGN.md §12.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};
use ferry_core::{DatastoreKind, EnvVar, Runtime, ServiceType, validate};

#[derive(Debug, Parser)]
#[command(
    name = "ferry",
    version,
    about = "Command-line client for Ferry, a self-hosted Render alternative",
    after_help = "Connection: --server/--token flags, else FERRY_SERVER/FERRY_TOKEN, else the config saved by 'ferry login'."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Default, Args)]
#[command(next_help_heading = "Global options")]
pub struct GlobalArgs {
    /// Ferry API URL, e.g. http://127.0.0.1:7878 [env: FERRY_SERVER]
    #[arg(long, global = true, value_name = "URL")]
    pub server: Option<String>,
    /// API token [env: FERRY_TOKEN]
    #[arg(long, global = true, value_name = "TOKEN")]
    pub token: Option<String>,
    /// Print the raw API JSON instead of formatted output
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Verify the server URL and token (from --server/--token) and save them
    Login,
    /// Show server information
    Info,
    /// List services
    #[command(visible_alias = "ls")]
    Services,
    /// Create a service
    Create(CreateArgs),
    /// Show a service's settings and state
    Show(NameArg),
    /// Change a service's settings
    Update(UpdateArgs),
    /// Delete a service with its containers and deploy history
    #[command(visible_alias = "rm")]
    Delete(DeleteArgs),
    /// Trigger a deploy
    Deploy(DeployArgs),
    /// Upload a local directory and deploy it (creates the service if needed)
    #[command(after_help = UP_AFTER_HELP)]
    Up(UpArgs),
    /// List a service's deploys, newest first
    Deploys(ListArgs),
    /// Cancel a queued or in-progress deploy
    Cancel(CancelArgs),
    /// Roll back to a previous deploy's image
    Rollback(RollbackArgs),
    /// Redeploy the live image with the current settings and environment
    Restart(FollowNameArgs),
    /// Stop a service's containers (the proxy answers 503)
    Suspend(NameArg),
    /// Resume a suspended service
    Resume(NameArg),
    /// Set the number of instances
    Scale(ScaleArgs),
    /// Show running instances with CPU and memory usage
    Status(NameArg),
    /// Show runtime, deploy or job logs
    Logs(LogsArgs),
    /// List or change a service's environment variables
    Env(EnvArgs),
    /// List or change a service's custom domains
    Domains(DomainsArgs),
    /// Run a one-off job (or trigger a cron job now)
    Run(RunArgs),
    /// List a service's job runs
    Jobs(ListArgs),
    /// Manage Postgres and Redis datastores
    #[command(subcommand)]
    Db(DbCommand),
    /// Manage environment groups
    #[command(subcommand, name = "env-group")]
    EnvGroup(EnvGroupCommand),
    /// Apply a ferry.yaml / render.yaml blueprint
    #[command(subcommand)]
    Blueprint(BlueprintCommand),
    /// Print (and try to open) a service's URL
    Open(NameArg),
}

const UP_AFTER_HELP: &str = "\
If the service doesn't exist, it is created with these flags. If it exists, the build & deploy \
settings (--runtime, --start-cmd, --port, ...) are applied to it first, like 'ferry update'; -e sets \
variables like 'ferry env set --no-restart'; --env-group links groups. Only --type is used solely \
when creating the service (an existing service keeps its type).";

#[derive(Debug, Clone, Args)]
pub struct NameArg {
    /// Service name or id
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct FollowNameArgs {
    /// Service name or id
    pub name: String,
    /// Stream the build/deploy logs until it finishes (exit 1 if it fails)
    #[arg(short, long)]
    pub follow: bool,
}

/// Build & deploy settings shared by `create`, `update` and `up`.
#[derive(Debug, Clone, Default, Args)]
pub struct SettingsArgs {
    /// Runtime: auto, docker, image, node, python, go, rust, ruby, static
    #[arg(long, value_name = "RUNTIME", value_parser = parse_runtime)]
    pub runtime: Option<Runtime>,
    /// Build context subdirectory (monorepos)
    #[arg(long = "root-dir", value_name = "DIR")]
    pub root_dir: Option<String>,
    /// Dockerfile path relative to the root dir
    #[arg(long, value_name = "PATH")]
    pub dockerfile: Option<String>,
    /// Build command (native runtimes)
    #[arg(long = "build-cmd", value_name = "CMD")]
    pub build_cmd: Option<String>,
    /// Start command (run with sh -c)
    #[arg(long = "start-cmd", value_name = "CMD")]
    pub start_cmd: Option<String>,
    /// Directory of built files to serve (static sites)
    #[arg(long = "publish-dir", value_name = "DIR")]
    pub publish_dir: Option<String>,
    /// Container port the app listens on
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
    /// Health check path, e.g. /healthz
    #[arg(long = "health", value_name = "PATH")]
    pub health: Option<String>,
    /// Number of instances
    #[arg(long, value_name = "N")]
    pub instances: Option<u32>,
    /// Cron schedule (cron jobs), e.g. "*/5 * * * *"
    #[arg(long, value_name = "CRON")]
    pub schedule: Option<String>,
    /// Mount a persistent disk at this path (limits the service to 1 instance)
    #[arg(long, value_name = "MOUNT")]
    pub disk: Option<String>,
    /// Custom domain (repeatable)
    #[arg(long = "domain", value_name = "DOMAIN")]
    pub domains: Vec<String>,
}

impl SettingsArgs {
    pub fn is_empty(&self) -> bool {
        self.runtime.is_none()
            && self.root_dir.is_none()
            && self.dockerfile.is_none()
            && self.build_cmd.is_none()
            && self.start_cmd.is_none()
            && self.publish_dir.is_none()
            && self.port.is_none()
            && self.health.is_none()
            && self.instances.is_none()
            && self.schedule.is_none()
            && self.disk.is_none()
            && self.domains.is_empty()
    }
}

#[derive(Debug, Clone, Args)]
pub struct CreateArgs {
    /// Service name (lowercase letters, digits and '-')
    pub name: String,
    /// Service type: web, pserv, worker, cron, static
    #[arg(short = 't', long = "type", value_name = "TYPE", value_parser = parse_service_type)]
    pub service_type: Option<ServiceType>,
    /// Git repository URL, or the path of a git repository on the server
    /// (a relative path that exists here is sent as an absolute path)
    #[arg(long, value_name = "URL", conflicts_with = "image")]
    pub repo: Option<String>,
    /// Git branch (default: main)
    #[arg(long, value_name = "BRANCH")]
    pub branch: Option<String>,
    /// Prebuilt Docker image
    #[arg(long, value_name = "IMAGE")]
    pub image: Option<String>,
    #[command(flatten)]
    pub settings: SettingsArgs,
    /// Environment variable (repeatable)
    #[arg(short = 'e', long = "env", value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    pub env: Vec<EnvVar>,
    /// Link an environment group (repeatable)
    #[arg(long = "env-group", value_name = "GROUP")]
    pub env_groups: Vec<String>,
    /// Don't redeploy on git push
    #[arg(long = "no-auto-deploy")]
    pub no_auto_deploy: bool,
    /// Don't queue the first deploy
    #[arg(long = "no-deploy", conflicts_with = "follow")]
    pub no_deploy: bool,
    /// Stream the first deploy's logs until it finishes (exit 1 if it fails)
    #[arg(short, long)]
    pub follow: bool,
}

#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    /// Service name or id
    pub name: String,
    /// Git repository URL or server-side path ("" clears; a relative path
    /// that exists here is sent as an absolute path)
    #[arg(long, value_name = "URL")]
    pub repo: Option<String>,
    /// Git branch
    #[arg(long, value_name = "BRANCH")]
    pub branch: Option<String>,
    /// Prebuilt Docker image ("" clears)
    #[arg(long, value_name = "IMAGE")]
    pub image: Option<String>,
    #[command(flatten)]
    pub settings: SettingsArgs,
    /// Redeploy on git push
    #[arg(long = "auto-deploy", overrides_with = "no_auto_deploy")]
    pub auto_deploy: bool,
    /// Don't redeploy on git push
    #[arg(long = "no-auto-deploy", overrides_with = "auto_deploy")]
    pub no_auto_deploy: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DeleteArgs {
    /// Service name or id
    pub name: String,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Delete even if other services still reference it (their next deploy or restart will fail)
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DeployArgs {
    /// Service name or id
    pub name: String,
    /// Build this exact commit (git services)
    #[arg(long, value_name = "SHA")]
    pub commit: Option<String>,
    /// Build without the Docker layer cache
    #[arg(long = "clear-cache")]
    pub clear_cache: bool,
    /// Stream the build/deploy logs until it finishes (exit 1 if it fails)
    #[arg(short, long)]
    pub follow: bool,
}

#[derive(Debug, Clone, Args)]
pub struct UpArgs {
    /// Service name or id (default: derived from the directory name)
    pub name: Option<String>,
    /// Directory to upload
    #[arg(short, long, value_name = "DIR", default_value = ".")]
    pub dir: PathBuf,
    /// Service type, only used when creating the service: web, pserv, worker, cron, static [default: web]
    #[arg(short = 't', long = "type", value_name = "TYPE", value_parser = parse_service_type)]
    pub service_type: Option<ServiceType>,
    /// Build & deploy settings: used to create the service, or applied to
    /// the existing one before deploying
    #[command(flatten)]
    pub settings: SettingsArgs,
    /// Environment variable, set before deploying (repeatable)
    #[arg(short = 'e', long = "env", value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    pub env: Vec<EnvVar>,
    /// Link an environment group before deploying (repeatable)
    #[arg(long = "env-group", value_name = "GROUP")]
    pub env_groups: Vec<String>,
    /// Build without the Docker layer cache
    #[arg(long = "clear-cache")]
    pub clear_cache: bool,
    /// Stream the build/deploy logs until it finishes (exit 1 if it fails)
    #[arg(short, long)]
    pub follow: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Service name or id
    pub name: String,
    /// Maximum number of entries
    #[arg(short = 'n', long, value_name = "N", default_value_t = 20)]
    pub limit: u32,
}

#[derive(Debug, Clone, Args)]
pub struct CancelArgs {
    /// Deploy id
    pub deploy_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RollbackArgs {
    /// Service name or id
    pub name: String,
    /// Deploy to roll back to
    pub deploy_id: String,
    /// Stream the deploy logs until it finishes (exit 1 if it fails)
    #[arg(short, long)]
    pub follow: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ScaleArgs {
    /// Service name or id
    pub name: String,
    /// Number of instances
    pub instances: u32,
}

#[derive(Debug, Clone, Args)]
#[command(group(ArgGroup::new("target").required(true).args(["name", "deploy", "job"])))]
pub struct LogsArgs {
    /// Service name or id (runtime logs)
    pub name: Option<String>,
    /// Show a deploy's build/deploy log instead
    #[arg(long, value_name = "DEPLOY_ID")]
    pub deploy: Option<String>,
    /// Show a job run's log instead
    #[arg(long, value_name = "JOB_ID")]
    pub job: Option<String>,
    /// Keep streaming new lines
    #[arg(short, long)]
    pub follow: bool,
    /// Number of recent lines per instance (runtime logs)
    #[arg(short = 'n', long, value_name = "N", conflicts_with_all = ["deploy", "job"])]
    pub tail: Option<u32>,
}

#[derive(Debug, Clone, Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct EnvArgs {
    #[command(subcommand)]
    pub command: Option<EnvCommand>,
    /// Service name or id (lists its variables)
    #[arg(required = true)]
    pub name: Option<String>,
    /// List the merged environment: linked env groups, then the service's own variables
    #[arg(long)]
    pub effective: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum EnvCommand {
    /// List variables as KEY=VALUE (values with newlines or quotes are
    /// printed double-quoted with backslash escapes)
    #[command(visible_alias = "list")]
    Ls(EnvLsArgs),
    /// Set variables (restarts the service unless --no-restart)
    Set(EnvSetArgs),
    /// Remove variables (restarts the service unless --no-restart)
    Unset(EnvUnsetArgs),
}

#[derive(Debug, Clone, Args)]
pub struct EnvLsArgs {
    /// Service name or id
    pub name: String,
    /// List the merged environment: linked env groups, then the service's own variables
    #[arg(long)]
    pub effective: bool,
}

#[derive(Debug, Clone, Args)]
pub struct EnvSetArgs {
    /// Service name or id
    pub name: String,
    /// KEY=VALUE pairs. Values may reference ${{datastore.NAME.connectionString}},
    /// ${{service.NAME.hostport}}, … (resolved when the service starts)
    #[arg(required = true, value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    pub vars: Vec<EnvVar>,
    /// Save without restarting
    #[arg(long = "no-restart")]
    pub no_restart: bool,
    /// Stream the restart's logs until it finishes (exit 1 if it fails)
    #[arg(short, long, conflicts_with = "no_restart")]
    pub follow: bool,
}

#[derive(Debug, Clone, Args)]
pub struct EnvUnsetArgs {
    /// Service name or id
    pub name: String,
    /// Variable names
    #[arg(required = true, value_name = "KEY", value_parser = parse_env_key)]
    pub keys: Vec<String>,
    /// Save without restarting
    #[arg(long = "no-restart")]
    pub no_restart: bool,
    /// Stream the restart's logs until it finishes (exit 1 if it fails)
    #[arg(short, long, conflicts_with = "no_restart")]
    pub follow: bool,
}

#[derive(Debug, Clone, Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct DomainsArgs {
    #[command(subcommand)]
    pub command: Option<DomainsCommand>,
    /// Service name or id (lists its custom domains)
    #[arg(required = true)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum DomainsCommand {
    /// List custom domains
    #[command(visible_alias = "list")]
    Ls(NameArg),
    /// Add a custom domain
    Add(DomainArgs),
    /// Remove a custom domain
    #[command(visible_alias = "remove")]
    Rm(DomainArgs),
}

#[derive(Debug, Clone, Args)]
pub struct DomainArgs {
    /// Service name or id
    pub name: String,
    /// Domain name, e.g. app.example.com
    pub domain: String,
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    /// Service name or id
    pub name: String,
    /// Stream the job's output until it finishes (exit 1 if it fails)
    #[arg(short, long)]
    pub follow: bool,
    /// Command to run after `--` (default for cron jobs: their start command)
    #[arg(last = true, value_name = "CMD")]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum DbCommand {
    /// Create a datastore
    Create(DbCreateArgs),
    /// List datastores
    #[command(visible_alias = "list")]
    Ls,
    /// Show a datastore with its connection URLs
    Show(DbNameArg),
    /// Delete a datastore and its data
    #[command(visible_alias = "delete")]
    Rm(DbRmArgs),
}

#[derive(Debug, Clone, Args)]
pub struct DbCreateArgs {
    /// Datastore name (also its hostname on the private network)
    pub name: String,
    /// postgres or redis
    #[arg(long, value_name = "KIND", default_value = "postgres", value_parser = parse_datastore_kind)]
    pub kind: DatastoreKind,
    /// Major version (default: 16 for postgres, 7 for redis)
    #[arg(long, value_name = "V")]
    pub version: Option<String>,
    /// Postgres database name
    #[arg(long, value_name = "NAME")]
    pub database: Option<String>,
    /// Postgres user
    #[arg(long = "user", value_name = "USER")]
    pub username: Option<String>,
    /// Wait until the datastore is available
    #[arg(short, long)]
    pub wait: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DbNameArg {
    /// Datastore name or id
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct DbRmArgs {
    /// Datastore name or id
    pub name: String,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Delete even if other services still reference it (their next deploy or restart will fail)
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum EnvGroupCommand {
    /// Create an env group, optionally with variables
    Create(EnvGroupCreateArgs),
    /// List env groups
    #[command(visible_alias = "list")]
    Ls,
    /// Show an env group's variables and linked services
    Show(GroupNameArg),
    /// Set variables (restarts linked services unless --no-restart)
    Set(EnvGroupSetArgs),
    /// Remove variables (restarts linked services unless --no-restart)
    Unset(EnvGroupUnsetArgs),
    /// Delete an env group
    #[command(visible_alias = "delete")]
    Rm(EnvGroupRmArgs),
    /// Link an env group to a service
    Link(LinkArgs),
    /// Unlink an env group from a service
    Unlink(LinkArgs),
}

#[derive(Debug, Clone, Args)]
pub struct EnvGroupCreateArgs {
    /// Group name
    pub name: String,
    /// KEY=VALUE pairs
    #[arg(value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    pub vars: Vec<EnvVar>,
}

#[derive(Debug, Clone, Args)]
pub struct GroupNameArg {
    /// Env group name or id
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct EnvGroupSetArgs {
    /// Env group name or id
    pub name: String,
    /// KEY=VALUE pairs
    #[arg(required = true, value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    pub vars: Vec<EnvVar>,
    /// Save without restarting linked services
    #[arg(long = "no-restart")]
    pub no_restart: bool,
}

#[derive(Debug, Clone, Args)]
pub struct EnvGroupUnsetArgs {
    /// Env group name or id
    pub name: String,
    /// Variable names
    #[arg(required = true, value_name = "KEY", value_parser = parse_env_key)]
    pub keys: Vec<String>,
    /// Save without restarting linked services
    #[arg(long = "no-restart")]
    pub no_restart: bool,
}

#[derive(Debug, Clone, Args)]
pub struct EnvGroupRmArgs {
    /// Env group name or id
    pub name: String,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Delete even if services are still linked to it
    #[arg(long)]
    pub force: bool,
    /// With --force: restart the linked live services so they drop the group's variables
    #[arg(long, requires = "force")]
    pub restart: bool,
}

#[derive(Debug, Clone, Args)]
pub struct LinkArgs {
    /// Service name or id
    pub service: String,
    /// Env group name or id
    pub group: String,
}

#[derive(Debug, Clone, Subcommand)]
pub enum BlueprintCommand {
    /// Create/update the resources described by a blueprint
    Apply(BlueprintApplyArgs),
}

#[derive(Debug, Clone, Args)]
pub struct BlueprintApplyArgs {
    /// Blueprint file (default: ./ferry.yaml, then ./render.yaml)
    pub file: Option<PathBuf>,
    /// Show what would change without changing anything
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Value parsers

pub fn parse_service_type(s: &str) -> Result<ServiceType, String> {
    s.parse().map_err(|_| format!("unknown service type '{s}' (expected web, pserv, worker, cron or static)"))
}

pub fn parse_runtime(s: &str) -> Result<Runtime, String> {
    s.parse().map_err(|e: ferry_core::Error| e.to_string())
}

pub fn parse_datastore_kind(s: &str) -> Result<DatastoreKind, String> {
    s.parse().map_err(|_| format!("unknown datastore kind '{s}' (expected postgres or redis)"))
}

/// `KEY=VALUE`, split on the first `=`; the value may be empty or contain `=`.
/// `${{…}}` references in the value must be well-formed (they are resolved
/// when the service starts; a malformed one could never resolve).
pub fn parse_env_pair(s: &str) -> Result<EnvVar, String> {
    let (key, value) = s.split_once('=').ok_or_else(|| format!("expected KEY=VALUE, got '{s}'"))?;
    validate::env_var(key, value).map_err(|e| e.to_string())?;
    crate::envref::check_syntax(value).map_err(|e| format!("{key}: {e}"))?;
    Ok(EnvVar::new(key, value))
}

pub fn parse_env_key(s: &str) -> Result<String, String> {
    validate::env_key(s).map_err(|e| e.to_string())?;
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        let mut full = vec!["ferry"];
        full.extend_from_slice(args);
        match Cli::try_parse_from(full) {
            Ok(c) => c,
            Err(e) => panic!("parse failed for {args:?}: {e}"),
        }
    }

    fn fails(args: &[&str]) -> clap::Error {
        let mut full = vec!["ferry"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).expect_err("expected a parse error")
    }

    #[test]
    fn clap_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_anywhere() {
        let c = parse(&["--server", "http://x:1", "services", "--token", "t", "--json"]);
        assert_eq!(c.global.server.as_deref(), Some("http://x:1"));
        assert_eq!(c.global.token.as_deref(), Some("t"));
        assert!(c.global.json);
        assert!(matches!(c.command, Command::Services));
        assert!(matches!(parse(&["ls"]).command, Command::Services));
        assert!(matches!(parse(&["login", "--server", "u", "--token", "t"]).command, Command::Login));
        assert!(matches!(parse(&["info"]).command, Command::Info));
    }

    #[test]
    fn create_maps_every_flag() {
        let c = parse(&[
            "create",
            "api",
            "--type",
            "pserv",
            "--repo",
            "https://github.com/a/b",
            "--branch",
            "dev",
            "--runtime",
            "nodejs",
            "--root-dir",
            "api",
            "--dockerfile",
            "Dockerfile.prod",
            "--build-cmd",
            "npm ci",
            "--start-cmd",
            "npm start",
            "--publish-dir",
            "dist",
            "--port",
            "3000",
            "--health",
            "/healthz",
            "--instances",
            "2",
            "--disk",
            "/data",
            "--domain",
            "a.example.com",
            "--domain",
            "b.example.com",
            "-e",
            "A=1",
            "--env",
            "URL=postgres://x?a=b",
            "--env-group",
            "shared",
            "--no-auto-deploy",
            "--follow",
        ]);
        let Command::Create(a) = c.command else { panic!("not create") };
        assert_eq!(a.name, "api");
        assert_eq!(a.service_type, Some(ServiceType::PrivateService));
        assert_eq!(a.repo.as_deref(), Some("https://github.com/a/b"));
        assert_eq!(a.branch.as_deref(), Some("dev"));
        assert_eq!(a.settings.runtime, Some(Runtime::Node));
        assert_eq!(a.settings.root_dir.as_deref(), Some("api"));
        assert_eq!(a.settings.dockerfile.as_deref(), Some("Dockerfile.prod"));
        assert_eq!(a.settings.build_cmd.as_deref(), Some("npm ci"));
        assert_eq!(a.settings.start_cmd.as_deref(), Some("npm start"));
        assert_eq!(a.settings.publish_dir.as_deref(), Some("dist"));
        assert_eq!(a.settings.port, Some(3000));
        assert_eq!(a.settings.health.as_deref(), Some("/healthz"));
        assert_eq!(a.settings.instances, Some(2));
        assert_eq!(a.settings.disk.as_deref(), Some("/data"));
        assert_eq!(a.settings.domains, vec!["a.example.com", "b.example.com"]);
        assert_eq!(a.env, vec![EnvVar::new("A", "1"), EnvVar::new("URL", "postgres://x?a=b")]);
        assert_eq!(a.env_groups, vec!["shared"]);
        assert!(a.no_auto_deploy && a.follow && !a.no_deploy);
    }

    #[test]
    fn create_type_aliases_and_errors() {
        for (alias, want) in [
            ("web", ServiceType::WebService),
            ("web_service", ServiceType::WebService),
            ("pserv", ServiceType::PrivateService),
            ("private", ServiceType::PrivateService),
            ("private_service", ServiceType::PrivateService),
            ("worker", ServiceType::BackgroundWorker),
            ("background_worker", ServiceType::BackgroundWorker),
            ("cron", ServiceType::CronJob),
            ("cron_job", ServiceType::CronJob),
            ("static", ServiceType::StaticSite),
            ("static_site", ServiceType::StaticSite),
        ] {
            let Command::Create(a) = parse(&["create", "x", "-t", alias]).command else { panic!() };
            assert_eq!(a.service_type, Some(want), "{alias}");
        }
        assert!(fails(&["create", "x", "--type", "lambda"]).to_string().contains("unknown service type"));
        assert!(fails(&["create", "x", "--runtime", "cobol"]).to_string().contains("invalid Runtime"));
        assert!(fails(&["create", "x", "--repo", "r", "--image", "i"]).to_string().contains("cannot be used"));
        assert!(fails(&["create", "x", "--no-deploy", "--follow"]).to_string().contains("cannot be used"));
        assert!(fails(&["create", "x", "--env", "NOEQUALS"]).to_string().contains("expected KEY=VALUE"));
        assert!(fails(&["create", "x", "--env", "=v"]).to_string().contains("invalid environment variable"));
        assert!(fails(&["create", "x", "--port", "70000"]).to_string().contains("70000"));
        assert!(fails(&["create"]).to_string().contains("<NAME>"));
    }

    #[test]
    fn update_flags_and_auto_deploy_toggle() {
        let Command::Update(a) =
            parse(&["update", "web", "--start-cmd", "", "--port", "0", "--auto-deploy", "--no-auto-deploy"]).command
        else {
            panic!()
        };
        assert_eq!(a.settings.start_cmd.as_deref(), Some(""));
        assert_eq!(a.settings.port, Some(0));
        assert!(a.no_auto_deploy && !a.auto_deploy, "last flag wins");
        let Command::Update(a) = parse(&["update", "web", "--no-auto-deploy", "--auto-deploy"]).command else {
            panic!()
        };
        assert!(a.auto_deploy && !a.no_auto_deploy);
        assert!(fails(&["update", "web", "--type", "worker"]).to_string().contains("--type"));
    }

    #[test]
    fn deploy_up_and_friends() {
        let Command::Deploy(a) = parse(&["deploy", "web", "--commit", "abc", "--clear-cache", "-f"]).command else {
            panic!()
        };
        assert_eq!((a.name.as_str(), a.commit.as_deref(), a.clear_cache, a.follow), ("web", Some("abc"), true, true));

        let Command::Up(a) = parse(&["up"]).command else { panic!() };
        assert_eq!(a.name, None);
        assert_eq!(a.dir, PathBuf::from("."));
        assert_eq!(a.service_type, None);
        let Command::Up(a) =
            parse(&["up", "site", "--dir", "./web", "--type", "static", "--publish-dir", "dist", "-e", "K=V", "-f"])
                .command
        else {
            panic!()
        };
        assert_eq!(a.name.as_deref(), Some("site"));
        assert_eq!(a.dir, PathBuf::from("./web"));
        assert_eq!(a.service_type, Some(ServiceType::StaticSite));
        assert_eq!(a.settings.publish_dir.as_deref(), Some("dist"));
        assert_eq!(a.env, vec![EnvVar::new("K", "V")]);
        assert!(a.follow);

        let Command::Deploys(a) = parse(&["deploys", "web"]).command else { panic!() };
        assert_eq!(a.limit, 20);
        let Command::Deploys(a) = parse(&["deploys", "web", "-n", "5"]).command else { panic!() };
        assert_eq!(a.limit, 5);
        let Command::Cancel(a) = parse(&["cancel", "dep-1"]).command else { panic!() };
        assert_eq!(a.deploy_id, "dep-1");
        let Command::Rollback(a) = parse(&["rollback", "web", "dep-1", "--follow"]).command else { panic!() };
        assert_eq!((a.name.as_str(), a.deploy_id.as_str(), a.follow), ("web", "dep-1", true));
        let Command::Restart(a) = parse(&["restart", "web"]).command else { panic!() };
        assert_eq!(a.name, "web");
        assert!(matches!(parse(&["suspend", "web"]).command, Command::Suspend(_)));
        assert!(matches!(parse(&["resume", "web"]).command, Command::Resume(_)));
        let Command::Scale(a) = parse(&["scale", "web", "3"]).command else { panic!() };
        assert_eq!(a.instances, 3);
        fails(&["scale", "web", "-1"]);
        fails(&["scale", "web"]);
        assert!(matches!(parse(&["status", "web"]).command, Command::Status(_)));
        assert!(matches!(parse(&["show", "web"]).command, Command::Show(_)));
        assert!(matches!(parse(&["open", "web"]).command, Command::Open(_)));
        let Command::Delete(a) = parse(&["delete", "web", "--yes"]).command else { panic!() };
        assert!(a.yes);
        let Command::Delete(a) = parse(&["rm", "web"]).command else { panic!() };
        assert!(!a.yes);
    }

    #[test]
    fn logs_targets() {
        let Command::Logs(a) = parse(&["logs", "web", "-f", "--tail", "50"]).command else { panic!() };
        assert_eq!((a.name.as_deref(), a.follow, a.tail), (Some("web"), true, Some(50)));
        let Command::Logs(a) = parse(&["logs", "--deploy", "dep-1", "-f"]).command else { panic!() };
        assert_eq!((a.deploy.as_deref(), a.follow), (Some("dep-1"), true));
        let Command::Logs(a) = parse(&["logs", "--job", "job-1"]).command else { panic!() };
        assert_eq!(a.job.as_deref(), Some("job-1"));
        fails(&["logs"]);
        fails(&["logs", "web", "--deploy", "dep-1"]);
        fails(&["logs", "--deploy", "d", "--job", "j"]);
        fails(&["logs", "--deploy", "d", "--tail", "5"]);
    }

    #[test]
    fn env_commands() {
        let Command::Env(a) = parse(&["env", "web"]).command else { panic!() };
        assert_eq!(a.name.as_deref(), Some("web"));
        assert!(a.command.is_none());
        let Command::Env(a) = parse(&["env", "set", "web", "A=1", "B=x=y", "C="]).command else { panic!() };
        let Some(EnvCommand::Set(s)) = a.command else { panic!() };
        assert_eq!(s.name, "web");
        assert_eq!(s.vars, vec![EnvVar::new("A", "1"), EnvVar::new("B", "x=y"), EnvVar::new("C", "")]);
        assert!(!s.no_restart);
        let Command::Env(a) = parse(&["env", "unset", "web", "A", "B", "--no-restart"]).command else { panic!() };
        let Some(EnvCommand::Unset(u)) = a.command else { panic!() };
        assert_eq!(u.keys, vec!["A", "B"]);
        assert!(u.no_restart);
        let Command::Env(a) = parse(&["env", "ls", "web"]).command else { panic!() };
        assert!(matches!(a.command, Some(EnvCommand::Ls(EnvLsArgs { effective: false, .. }))));
        let Command::Env(a) = parse(&["env", "ls", "web", "--effective"]).command else { panic!() };
        assert!(matches!(a.command, Some(EnvCommand::Ls(EnvLsArgs { effective: true, .. }))));
        let Command::Env(a) = parse(&["env", "web", "--effective"]).command else { panic!() };
        assert!(a.effective && a.command.is_none());
        let Command::Env(a) = parse(&["env", "set", "web", "A=1", "-f"]).command else { panic!() };
        let Some(EnvCommand::Set(s)) = a.command else { panic!() };
        assert!(s.follow);
        fails(&["env", "set", "web", "A=1", "--no-restart", "--follow"]);
        fails(&["env", "unset", "web", "A", "--no-restart", "-f"]);
        // References that could never resolve are refused before anything is sent.
        let err = fails(&["env", "set", "web", "TPL=Hello ${{ name }}"]).to_string();
        assert!(err.contains("TPL: invalid reference") && err.contains("kind.name.property"), "{err}");
        fails(&["env", "set", "web", "X=${{bucket.b.url}}"]);
        let Command::Env(a) = parse(&["env", "set", "web", "DB=${{datastore.pgx.connectionString}}"]).command else {
            panic!()
        };
        assert!(matches!(a.command, Some(EnvCommand::Set(_))), "unknown targets are checked by the server");
        fails(&["env"]);
        fails(&["env", "set", "web"]);
        fails(&["env", "set", "web", "NOPE"]);
        fails(&["env", "unset", "web", "BAD KEY"]);
    }

    #[test]
    fn domains_commands() {
        let Command::Domains(a) = parse(&["domains", "web"]).command else { panic!() };
        assert_eq!(a.name.as_deref(), Some("web"));
        let Command::Domains(a) = parse(&["domains", "add", "web", "app.example.com"]).command else { panic!() };
        let Some(DomainsCommand::Add(d)) = a.command else { panic!() };
        assert_eq!((d.name.as_str(), d.domain.as_str()), ("web", "app.example.com"));
        let Command::Domains(a) = parse(&["domains", "rm", "web", "app.example.com"]).command else { panic!() };
        assert!(matches!(a.command, Some(DomainsCommand::Rm(_))));
        fails(&["domains"]);
        fails(&["domains", "add", "web"]);
    }

    #[test]
    fn run_and_jobs() {
        let Command::Run(a) =
            parse(&["run", "api", "--follow", "--", "python", "manage.py", "migrate", "--noinput"]).command
        else {
            panic!()
        };
        assert!(a.follow);
        assert_eq!(a.command, vec!["python", "manage.py", "migrate", "--noinput"]);
        let Command::Run(a) = parse(&["run", "nightly"]).command else { panic!() };
        assert!(a.command.is_empty() && !a.follow);
        fails(&["run", "api", "echo"]);
        let Command::Jobs(a) = parse(&["jobs", "nightly"]).command else { panic!() };
        assert_eq!(a.name, "nightly");
    }

    #[test]
    fn db_commands() {
        let Command::Db(DbCommand::Create(a)) = parse(&["db", "create", "main"]).command else { panic!() };
        assert_eq!((a.kind, a.version.as_deref()), (DatastoreKind::Postgres, None));
        let Command::Db(DbCommand::Create(a)) =
            parse(&["db", "create", "cache", "--kind", "keyvalue", "--version", "7", "--wait"]).command
        else {
            panic!()
        };
        assert_eq!((a.kind, a.version.as_deref(), a.wait), (DatastoreKind::Redis, Some("7"), true));
        let Command::Db(DbCommand::Create(a)) =
            parse(&["db", "create", "app-db", "--database", "app", "--user", "app"]).command
        else {
            panic!()
        };
        assert_eq!((a.database.as_deref(), a.username.as_deref()), (Some("app"), Some("app")));
        assert!(matches!(parse(&["db", "ls"]).command, Command::Db(DbCommand::Ls)));
        assert!(matches!(parse(&["db", "show", "main"]).command, Command::Db(DbCommand::Show(_))));
        let Command::Db(DbCommand::Rm(a)) = parse(&["db", "rm", "main", "-y"]).command else { panic!() };
        assert!(a.yes);
        fails(&["db", "create", "x", "--kind", "mysql"]);
    }

    #[test]
    fn env_group_commands() {
        let Command::EnvGroup(EnvGroupCommand::Create(a)) = parse(&["env-group", "create", "shared", "A=1"]).command
        else {
            panic!()
        };
        assert_eq!((a.name.as_str(), a.vars.len()), ("shared", 1));
        let Command::EnvGroup(EnvGroupCommand::Create(a)) = parse(&["env-group", "create", "empty"]).command else {
            panic!()
        };
        assert!(a.vars.is_empty());
        assert!(matches!(parse(&["env-group", "ls"]).command, Command::EnvGroup(EnvGroupCommand::Ls)));
        assert!(matches!(parse(&["env-group", "show", "g"]).command, Command::EnvGroup(EnvGroupCommand::Show(_))));
        let Command::EnvGroup(EnvGroupCommand::Set(a)) =
            parse(&["env-group", "set", "g", "A=1", "--no-restart"]).command
        else {
            panic!()
        };
        assert!(a.no_restart);
        let Command::EnvGroup(EnvGroupCommand::Unset(a)) = parse(&["env-group", "unset", "g", "A"]).command else {
            panic!()
        };
        assert_eq!(a.keys, vec!["A"]);
        let Command::EnvGroup(EnvGroupCommand::Rm(a)) = parse(&["env-group", "rm", "g", "--yes"]).command else {
            panic!()
        };
        assert!(a.yes);
        let Command::EnvGroup(EnvGroupCommand::Link(a)) = parse(&["env-group", "link", "web", "g"]).command else {
            panic!()
        };
        assert_eq!((a.service.as_str(), a.group.as_str()), ("web", "g"));
        assert!(matches!(
            parse(&["env-group", "unlink", "web", "g"]).command,
            Command::EnvGroup(EnvGroupCommand::Unlink(_))
        ));
        fails(&["env-group", "set", "g"]);
    }

    #[test]
    fn blueprint_apply() {
        let Command::Blueprint(BlueprintCommand::Apply(a)) = parse(&["blueprint", "apply"]).command else { panic!() };
        assert!(a.file.is_none() && !a.dry_run);
        let Command::Blueprint(BlueprintCommand::Apply(a)) =
            parse(&["blueprint", "apply", "render.yaml", "--dry-run"]).command
        else {
            panic!()
        };
        assert_eq!(a.file, Some(PathBuf::from("render.yaml")));
        assert!(a.dry_run);
    }

    #[test]
    fn env_pair_parsing() {
        assert_eq!(parse_env_pair("A=b=c").unwrap(), EnvVar::new("A", "b=c"));
        assert_eq!(parse_env_pair("A=").unwrap(), EnvVar::new("A", ""));
        assert_eq!(parse_env_pair("A= spaced ").unwrap(), EnvVar::new("A", " spaced "));
        assert!(parse_env_pair("A").is_err());
        assert!(parse_env_pair("=x").is_err());
        assert!(parse_env_pair("A B=x").is_err());
        assert_eq!(
            parse_env_pair("DB=${{ datastore.main.connectionString }}").unwrap().value,
            "${{ datastore.main.connectionString }}"
        );
        assert!(parse_env_pair("DB=${{datastore.main}}").unwrap_err().contains("DB: invalid reference"));
        assert!(parse_env_pair(&format!("BIG={}", "x".repeat(validate::MAX_ENV_VALUE_BYTES + 1))).is_err());
    }
}
