//! Command-line syntax (clap derive). See DESIGN.md §19.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ferry_core::resources;

/// The data directory `ferryd` uses when none is given.
pub const DEFAULT_DATA_DIR: &str = "./ferry-data";

/// Ferry server: a self-hosted Render alternative.
///
/// Without a command, `ferryd` starts the server in the background when it
/// is run from a terminal (`ferryd start`), and in the foreground otherwise
/// (`ferryd run`): under a service manager, in a container, in a pipe.
#[derive(Debug, Parser)]
#[command(name = "ferryd", version, about, long_about, args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub server: ServerArgs,
    /// Print the OpenAPI document of the API and exit (used to generate the docs site).
    #[arg(long, hide = true)]
    pub dump_openapi: bool,
    /// Print this command-line reference as Markdown and exit (used to generate the docs site).
    #[arg(long, hide = true)]
    pub dump_markdown_help: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the server in the background.
    ///
    /// Gives the terminal back once the server listens, or says why it
    /// didn't start. The server's output goes to <data-dir>/ferryd.log.
    Start(ServerArgs),
    /// Run the server in the foreground.
    ///
    /// Until Ctrl-C or SIGTERM: for a service manager (systemd, launchd), a
    /// container, or to watch the server's log in the terminal.
    Run {
        #[command(flatten)]
        server: ServerArgs,
        /// This process was started by `ferryd start`.
        #[arg(long, hide = true)]
        detached: bool,
    },
    /// Stop the server.
    ///
    /// Asks the server that uses the data directory to shut down, and waits
    /// until it has. Running it again while the server shuts down forces the
    /// server to exit at once, like a second Ctrl-C.
    Stop(DataDir),
    /// Say whether the server runs.
    ///
    /// Prints where the server that uses the data directory listens. The
    /// exit code is 0 when a server runs and 3 when none does.
    Status(DataDir),
    /// Replace the password of the server's account.
    ///
    /// For an administrator who forgot it: asks for the new password (or
    /// reads it from standard input when that isn't a terminal) and signs
    /// every browser out. Works while the server runs.
    ResetPassword(DataDir),
    /// Print the log of a server started in the background.
    Logs {
        #[command(flatten)]
        data_dir: DataDir,
        /// Keep printing what the server logs, until Ctrl-C.
        #[arg(short, long)]
        follow: bool,
        /// Lines to print from the end of the log.
        #[arg(short = 'n', long, value_name = "N", default_value_t = 100)]
        lines: usize,
    },
}

/// The data directory of the server a command is about.
#[derive(Debug, clap::Args)]
pub struct DataDir {
    /// Data directory of the server.
    #[arg(long, env = "FERRY_DATA_DIR", default_value = DEFAULT_DATA_DIR)]
    pub data_dir: PathBuf,
}

/// The options of the server (`ferryd`, `ferryd start`, `ferryd run`).
#[derive(Debug, clap::Args)]
pub struct ServerArgs {
    /// Directory for the database, logs, build scratch space and certificates.
    #[arg(long, env = "FERRY_DATA_DIR", default_value = DEFAULT_DATA_DIR)]
    pub data_dir: PathBuf,
    /// API + dashboard listen address.
    #[arg(long, env = "FERRY_API_ADDR", default_value = "127.0.0.1:7878")]
    pub api_addr: SocketAddr,
    /// Public HTTP proxy listen address.
    #[arg(long, env = "FERRY_PROXY_ADDR", default_value = "0.0.0.0:8080")]
    pub proxy_addr: SocketAddr,
    /// Public HTTPS proxy listen address (enables TLS together with --acme-email).
    #[arg(long, env = "FERRY_HTTPS_ADDR")]
    pub https_addr: Option<SocketAddr>,
    /// Services are reachable at <name>.<base-domain>. More domains are
    /// connected while the server runs (dashboard: Server → Domains, or
    /// `ferry domains connect`).
    #[arg(long, env = "FERRY_BASE_DOMAIN", default_value = "localhost")]
    pub base_domain: String,
    /// Public address of this server: the value of the DNS records shown for
    /// a domain, and what a domain is expected to resolve to. Repeat it (or
    /// separate with commas) for an IPv4 and an IPv6 address. Default: its
    /// IPv4 address is found out — the address of the outbound interface
    /// when it is a public one, else asked from api.ipify.org,
    /// ipv4.icanhazip.com or checkip.amazonaws.com.
    #[arg(long = "public-ip", env = "FERRY_PUBLIC_IP", value_name = "IP", value_delimiter = ',')]
    pub public_ip: Vec<IpAddr>,
    /// Port shown in public URLs (defaults to the proxy port).
    #[arg(long, env = "FERRY_PUBLIC_PORT")]
    pub public_port: Option<u16>,
    /// Host that serves the dashboard + API through the public proxy ("none"
    /// to disable). Default: ferry.<base-domain> when the base domain is local
    /// (e.g. localhost), otherwise disabled — enable it explicitly, ideally
    /// with HTTPS, since it exposes the admin API.
    #[arg(long, env = "FERRY_DASHBOARD_HOST")]
    pub dashboard_host: Option<String>,
    /// Prefix for Docker resources; lets several Ferry servers share one daemon.
    #[arg(long, env = "FERRY_NAME_PREFIX", default_value = "ferry")]
    pub name_prefix: String,
    /// The server token: an API token that always works, for the CLI and
    /// automation on the server itself. Default: read from / generated into
    /// <data-dir>/api_token.
    #[arg(long, env = "FERRY_API_TOKEN", hide_env_values = true)]
    pub api_token: Option<String>,
    /// Secret of the GitHub webhooks added to repositories by hand (POST
    /// /hooks/github). A GitHub account connected in the dashboard from a
    /// public address needs none: its pushes arrive signed by its own app.
    #[arg(long, env = "FERRY_GITHUB_WEBHOOK_SECRET", hide_env_values = true)]
    pub github_webhook_secret: Option<String>,
    /// Email for Let's Encrypt; enables automatic HTTPS with --https-addr.
    #[arg(long, env = "FERRY_ACME_EMAIL")]
    pub acme_email: Option<String>,
    /// Use the Let's Encrypt staging environment.
    #[arg(long, env = "FERRY_ACME_STAGING")]
    pub acme_staging: bool,
    /// Custom ACME directory URL (e.g. Pebble for testing).
    #[arg(long, env = "FERRY_ACME_DIRECTORY")]
    pub acme_directory: Option<String>,
    /// Maximum concurrent builds.
    #[arg(long, env = "FERRY_BUILD_CONCURRENCY", default_value_t = 2)]
    pub build_concurrency: usize,
    /// Container port used when nothing else specifies one.
    #[arg(long, env = "FERRY_DEFAULT_PORT", default_value_t = 10000)]
    pub default_port: u16,
    /// Built images kept per service (for rollbacks).
    #[arg(long, env = "FERRY_KEEP_IMAGES", default_value_t = 5)]
    pub keep_images: usize,
    /// Finished job runs (and their logs) kept per service.
    #[arg(long, env = "FERRY_KEEP_JOB_RUNS", default_value_t = 100)]
    pub keep_job_runs: usize,
    /// docker CLI used for builds.
    #[arg(long, env = "FERRY_DOCKER_BIN", default_value = "docker")]
    pub docker_bin: String,
    /// Seconds a new deploy has to pass its health check.
    #[arg(long, env = "FERRY_HEALTH_TIMEOUT", default_value_t = 120)]
    pub health_check_timeout: u64,
    /// Host name printed in external datastore connection strings.
    #[arg(long, env = "FERRY_ADVERTISE_HOST", default_value = "127.0.0.1")]
    pub advertise_host: String,
    /// Memory limit of each service, job and datastore container that sets
    /// none of its own (e.g. 512M, 1G; 0 = unlimited).
    #[arg(long, env = "FERRY_DEFAULT_MEMORY_LIMIT", value_name = "SIZE")]
    #[arg(default_value = "512M", value_parser = parse_default_memory)]
    pub default_memory_limit: u32,
    /// CPU limit of each service, job and datastore container that sets none
    /// of its own (e.g. 0.5, 2, 500m; 0 = unlimited).
    #[arg(long, env = "FERRY_DEFAULT_CPU_LIMIT", value_name = "CPUS")]
    #[arg(default_value = "1", value_parser = parse_default_cpus)]
    pub default_cpu_limit: f64,
    /// Max processes + threads per container, a fork-bomb guard (0 = unlimited).
    #[arg(long, env = "FERRY_PIDS_LIMIT", value_name = "N", default_value_t = 1024)]
    pub pids_limit: u32,
    /// Rotate each container's Docker log at this size, with the json-file
    /// driver (e.g. 10M; 0 = keep the Docker daemon's log configuration). A
    /// daemon whose default log driver isn't json-file keeps its driver.
    #[arg(long, env = "FERRY_LOG_MAX_SIZE", value_name = "SIZE", default_value = "10M")]
    #[arg(value_parser = parse_size)]
    pub log_max_size: u32,
    /// Log files kept per container when rotating (current one included).
    #[arg(long, env = "FERRY_LOG_MAX_FILES", value_name = "N", default_value_t = 3)]
    #[arg(value_parser = clap::value_parser!(u32).range(1..))]
    pub log_max_files: u32,
    /// Deploys and new datastores fail early when less disk than this is free
    /// on the data directory's (or the local Docker root's) filesystem (e.g.
    /// 1G; 0 = no check). Recreating an existing datastore's container isn't
    /// blocked.
    #[arg(long, env = "FERRY_MIN_FREE_DISK", value_name = "SIZE", default_value = "1G")]
    #[arg(value_parser = parse_size)]
    pub min_free_disk: u32,
    /// Linux: OOM-killer score adjustment of ferryd itself, -1000..=1000
    /// (negative = killed after the containers when the host runs out of
    /// memory; 0 = leave unchanged). Lowering it needs root or
    /// CAP_SYS_RESOURCE (or OOMScoreAdjust= in a systemd unit).
    #[arg(long, env = "FERRY_OOM_SCORE_ADJ", value_name = "N", default_value_t = -500)]
    #[arg(allow_negative_numbers = true, value_parser = clap::value_parser!(i32).range(-1000..=1000))]
    pub oom_score_adj: i32,
    /// Take ownership of Docker resources (with this name prefix) that another
    /// Ferry data directory owns — e.g. after restoring a backup into a new data
    /// directory or losing the old one (moving a data directory keeps ownership).
    #[arg(long)]
    pub take_over: bool,
}

/// `--log-max-size` / `--min-free-disk`: a size in MiB (`10M`, `1G`, `0`).
fn parse_size(s: &str) -> Result<u32, String> {
    resources::parse_memory_mb(s).map_err(|e| e.to_string())
}

/// `--default-memory-limit`: a size in MiB within the accepted limit range,
/// or 0 (unlimited).
fn parse_default_memory(s: &str) -> Result<u32, String> {
    let mb = parse_size(s)?;
    if mb > 0 {
        resources::validate_memory_mb(mb).map_err(|e| e.to_string())?;
    }
    Ok(mb)
}

/// `--default-cpu-limit`: CPUs within the accepted limit range, or 0
/// (unlimited).
fn parse_default_cpus(s: &str) -> Result<f64, String> {
    // (A non-zero amount that rounds to 0, like `0.004`, is refused by
    // parse_cpus: it must not silently mean "unlimited".)
    let cpus = resources::parse_cpus(s).map_err(|e| e.to_string())?;
    if cpus > 0.0 {
        resources::validate_cpus(cpus).map_err(|e| e.to_string())?;
    }
    Ok(cpus)
}

/// Is `ferryd` run by a person at a terminal? Then a bare `ferryd` gives the
/// terminal back. Not as PID 1: a container ends with its first process.
pub fn interactive() -> bool {
    cfg!(unix) && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && std::process::id() != 1
}

/// The options to pass on to the `ferryd run` that `ferryd start` executes:
/// the command line as it was typed, without the `start` command.
pub fn server_args(argv: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = argv.into_iter().skip(1).collect();
    if args.first().is_some_and(|a| a == "start") {
        args.remove(0);
    }
    args
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::CommandFactory;
    use ferry_core::Config;

    use super::*;

    fn parse_cli(extra: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("ferryd").chain(extra.iter().copied()))
    }

    /// The server options of a command line without a command.
    fn parse(extra: &[&str]) -> Result<ServerArgs, clap::Error> {
        parse_cli(extra).map(|cli| cli.server)
    }

    /// The first line of a clap error (the rest is usage help).
    fn error_line(extra: &[&str]) -> String {
        let err = parse(extra).expect_err("the flags should be rejected");
        err.to_string().lines().next().unwrap_or_default().to_string()
    }

    #[test]
    fn commands_parse() {
        // No command: the options of the server.
        let cli = parse_cli(&["--data-dir", "/x"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.server.data_dir, Path::new("/x"));
        assert_eq!(parse(&[]).unwrap().data_dir, Path::new(DEFAULT_DATA_DIR));

        // `start` and `run` take the same options.
        match parse_cli(&["start", "--api-addr", "127.0.0.1:9000"]).unwrap().command {
            Some(Command::Start(args)) => assert_eq!(args.api_addr.port(), 9000),
            other => panic!("{other:?}"),
        }
        match parse_cli(&["run", "--api-addr", "127.0.0.1:9000"]).unwrap().command {
            Some(Command::Run { server, detached }) => {
                assert_eq!(server.api_addr.port(), 9000);
                assert!(!detached);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_cli(&["run", "--detached"]).unwrap().command,
            Some(Command::Run { detached: true, .. })
        ));

        // The others only know the data directory.
        match parse_cli(&["stop"]).unwrap().command {
            Some(Command::Stop(dir)) => assert_eq!(dir.data_dir, Path::new(DEFAULT_DATA_DIR)),
            other => panic!("{other:?}"),
        }
        match parse_cli(&["status", "--data-dir", "/var/lib/ferry"]).unwrap().command {
            Some(Command::Status(dir)) => assert_eq!(dir.data_dir, Path::new("/var/lib/ferry")),
            other => panic!("{other:?}"),
        }
        match parse_cli(&["logs"]).unwrap().command {
            Some(Command::Logs { follow, lines, .. }) => assert_eq!((follow, lines), (false, 100)),
            other => panic!("{other:?}"),
        }
        match parse_cli(&["logs", "-f", "-n", "20", "--data-dir", "/x"]).unwrap().command {
            Some(Command::Logs { data_dir, follow, lines }) => {
                assert_eq!((follow, lines), (true, 20));
                assert_eq!(data_dir.data_dir, Path::new("/x"));
            }
            other => panic!("{other:?}"),
        }
        match parse_cli(&["reset-password", "--data-dir", "/var/lib/ferry"]).unwrap().command {
            Some(Command::ResetPassword(dir)) => assert_eq!(dir.data_dir, Path::new("/var/lib/ferry")),
            other => panic!("{other:?}"),
        }
        assert!(parse_cli(&["stop", "--api-addr", "127.0.0.1:9000"]).is_err());
        // An option in front of a command is refused, not silently dropped.
        assert!(parse_cli(&["--data-dir", "/x", "stop"]).is_err());
        assert!(parse_cli(&["--api-addr", "127.0.0.1:9000", "run"]).is_err());
    }

    #[test]
    fn start_hands_its_options_to_run() {
        let args = |argv: &[&str]| server_args(argv.iter().map(OsString::from));
        assert_eq!(args(&["ferryd"]), Vec::<OsString>::new());
        assert_eq!(args(&["ferryd", "--data-dir", "/x"]), ["--data-dir", "/x"]);
        assert_eq!(args(&["ferryd", "start", "--data-dir", "/x"]), ["--data-dir", "/x"]);
        assert_eq!(args(&["ferryd", "start", "--data-dir", "start"]), ["--data-dir", "start"]);
        // What `start` executes parses as the same server.
        let run = parse_cli(&["run", "--detached", "--data-dir", "/x"]).unwrap();
        assert!(
            matches!(run.command, Some(Command::Run { detached: true, server }) if server.data_dir == Path::new("/x"))
        );
    }

    #[test]
    fn limit_flag_defaults() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.default_memory_limit, 512);
        assert_eq!(args.default_cpu_limit, 1.0);
        assert_eq!(args.pids_limit, 1024);
        assert_eq!(args.log_max_size, 10);
        assert_eq!(args.log_max_files, 3);
        assert_eq!(args.min_free_disk, 1024);
        assert_eq!(args.oom_score_adj, -500);
        // Same defaults as the library's Config.
        let defaults = Config::default();
        assert_eq!(args.default_memory_limit, defaults.default_memory_limit_mb);
        assert_eq!(args.default_cpu_limit, defaults.default_cpu_limit);
        assert_eq!(args.pids_limit, defaults.pids_limit);
        assert_eq!(args.log_max_size, defaults.log_max_size_mb);
        assert_eq!(args.log_max_files, defaults.log_max_files);
        assert_eq!(u64::from(args.min_free_disk), defaults.min_free_disk_mb);
    }

    #[test]
    fn limit_flags_parse_human_sizes() {
        let args = parse(&[
            "--default-memory-limit",
            "1.5G",
            "--default-cpu-limit",
            "500m",
            "--pids-limit",
            "256",
            "--log-max-size",
            "50M",
            "--log-max-files",
            "5",
            "--min-free-disk",
            "10G",
            "--oom-score-adj",
            "-900",
        ])
        .unwrap();
        assert_eq!(args.default_memory_limit, 1536);
        assert_eq!(args.default_cpu_limit, 0.5);
        assert_eq!(args.pids_limit, 256);
        assert_eq!(args.log_max_size, 50);
        assert_eq!(args.log_max_files, 5);
        assert_eq!(args.min_free_disk, 10240);
        assert_eq!(args.oom_score_adj, -900);

        // 0 turns each one off.
        let off = parse(&[
            "--default-memory-limit=0",
            "--default-cpu-limit=0",
            "--pids-limit=0",
            "--log-max-size=0",
            "--min-free-disk=0",
            "--oom-score-adj=0",
        ])
        .unwrap();
        assert_eq!(
            (off.default_memory_limit, off.default_cpu_limit, off.pids_limit, off.log_max_size, off.min_free_disk),
            (0, 0.0, 0, 0, 0)
        );
        assert_eq!(off.oom_score_adj, 0);
        assert_eq!(
            parse(&["--default-memory-limit", "2048", "--default-cpu-limit", "2"]).unwrap().default_memory_limit,
            2048
        );
        assert_eq!(parse(&["--oom-score-adj", "300"]).unwrap().oom_score_adj, 300);
    }

    #[test]
    fn limit_flags_are_validated() {
        let line = error_line(&["--default-memory-limit", "8M"]);
        assert!(line.contains("--default-memory-limit") && line.contains("between 16 MiB and 1024 GiB"), "{line}");
        let line = error_line(&["--default-memory-limit", "lots"]);
        assert!(line.contains("invalid memory size 'lots'"), "{line}");
        let line = error_line(&["--default-memory-limit", "2T"]);
        assert!(line.contains("between 16 MiB and 1024 GiB"), "{line}");
        let line = error_line(&["--default-cpu-limit", "1000"]);
        assert!(line.contains("--default-cpu-limit") && line.contains("between 0.01 and 512 CPUs"), "{line}");
        let line = error_line(&["--default-cpu-limit", "0.004"]);
        assert!(line.contains("between 0.01 and 512 CPUs (got 0.004)"), "rounding must not mean unlimited: {line}");
        let line = error_line(&["--default-cpu-limit=-1"]);
        assert!(line.contains("--default-cpu-limit") && line.contains("invalid CPU amount '-1'"), "{line}");
        let line = error_line(&["--log-max-files", "0"]);
        assert!(line.contains("--log-max-files"), "{line}");
        let line = error_line(&["--log-max-size", "10X"]);
        assert!(line.contains("invalid memory size '10X'"), "{line}");
        let line = error_line(&["--min-free-disk=-1G"]);
        assert!(line.contains("--min-free-disk") && line.contains("invalid memory size '-1G'"), "{line}");
        for bad in ["-1001", "1001", "x"] {
            let line = error_line(&["--oom-score-adj", bad]);
            assert!(line.contains("--oom-score-adj"), "{bad}: {line}");
        }
        assert!(parse(&["--pids-limit", "-1"]).is_err());
    }

    #[test]
    fn limit_flags_have_env_vars() {
        let command = Cli::command();
        for (id, env) in [
            ("default_memory_limit", "FERRY_DEFAULT_MEMORY_LIMIT"),
            ("default_cpu_limit", "FERRY_DEFAULT_CPU_LIMIT"),
            ("pids_limit", "FERRY_PIDS_LIMIT"),
            ("log_max_size", "FERRY_LOG_MAX_SIZE"),
            ("log_max_files", "FERRY_LOG_MAX_FILES"),
            ("min_free_disk", "FERRY_MIN_FREE_DISK"),
            ("oom_score_adj", "FERRY_OOM_SCORE_ADJ"),
        ] {
            let arg = command.get_arguments().find(|a| a.get_id() == id).unwrap_or_else(|| panic!("no flag {id}"));
            assert_eq!(arg.get_env().and_then(|e| e.to_str()), Some(env), "{id}");
        }
        command.debug_assert();
    }
}
