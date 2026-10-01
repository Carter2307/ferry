//! Dockerfile generation for native runtimes (Nixpacks-style, small official
//! base images, dependency layers cached separately from the source).
//!
//! # Build-time environment
//!
//! Service env vars are never declared with `ARG`: BuildKit records ARG
//! values in the image history, so every secret would travel with the image
//! (`docker history`, `docker save`). Each usable variable is instead exposed
//! only to the steps that run project code — tool and dependency installs,
//! the build command — through a BuildKit secret mount
//! (`RUN --mount=type=secret,id=KEY,env=KEY …`); the builder hands the values
//! to `docker build --secret` through its environment. Secret values are not
//! part of BuildKit's cache key, so a keyed digest of them is declared as
//! `ARG FERRY_BUILD_ENV_DIGEST` before those steps: changing a variable still
//! re-runs them.

use std::collections::BTreeSet;
use std::path::Path;

use ferry_core::{Error, Result, Runtime, ServiceType};
use serde_json::Value;

use crate::detect::{self, CargoInfo, node_major_for, read_text};
use crate::{DockerfileOptions, GeneratedDockerfile, StartCommand};

pub(crate) const DEFAULT_PYTHON: &str = "3.12";
pub(crate) const DEFAULT_GO: &str = "1.23";
pub(crate) const DEFAULT_RUBY: &str = "3.3";
const NGINX_HTML: &str = "/usr/share/nginx/html";
/// Build arg carrying a keyed digest of the build-time environment (see the
/// module docs); it keeps the layer cache honest without exposing values.
pub(crate) const ENV_DIGEST_ARG: &str = "FERRY_BUILD_ENV_DIGEST";
/// Variable names Ferry uses for itself during builds (the digest arg, the
/// secret sources handed to the docker CLI).
pub(crate) const RESERVED_PREFIX: &str = "FERRY_BUILD_";
/// Alpine packages for Hugo sites (git for themes/modules).
const HUGO_APK: &str = "apk add --no-cache git hugo";

/// Entry point behind [`crate::generate_dockerfile`].
pub(crate) fn generate(runtime: Runtime, dir: &Path, opts: &DockerfileOptions) -> Result<GeneratedDockerfile> {
    let runtime = match runtime {
        Runtime::Auto => match detect::detect(dir) {
            Some(r) => r,
            None if opts.service_type == Some(ServiceType::StaticSite) => Runtime::Static,
            None => {
                return Err(Error::invalid(
                    "could not detect the runtime: no Dockerfile or known project manifest found",
                ));
            }
        },
        r => r,
    };
    if runtime == Runtime::Static || opts.service_type == Some(ServiceType::StaticSite) {
        return static_site(dir, runtime, opts);
    }
    check_sources(runtime, dir, opts)?;
    let (contents, start) = match runtime {
        Runtime::Node => node(dir, opts)?,
        Runtime::Python => python(dir, opts)?,
        Runtime::Go => go(dir, opts)?,
        Runtime::Rust => rust(dir, opts)?,
        Runtime::Ruby => ruby(dir, opts)?,
        Runtime::Docker | Runtime::Image | Runtime::Auto | Runtime::Static => {
            return Err(Error::invalid(format!("no Dockerfile generator for runtime '{runtime}'")));
        }
    };
    Ok(GeneratedDockerfile { contents, port_hint: None, start })
}

/// A generated Dockerfile, and the command it starts (`None` for a cron job
/// whose command comes at run time).
type Generated = (String, Option<StartCommand>);

/// A runtime chosen explicitly must match the source: fail with a clear
/// message instead of deep inside `npm` / `bundle` / `go build`, or with a
/// build that "succeeds" and an app that cannot start. A build command does
/// not lift the check (the generated images still expect the runtime's
/// project layout); it only lets Go build loose `.go` files without go.mod.
fn check_sources(runtime: Runtime, dir: &Path, opts: &DockerfileOptions) -> Result<()> {
    let has = |f: &str| dir.join(f).is_file();
    let custom_build = nonempty(&opts.build_command).is_some();
    let missing = match runtime {
        Runtime::Node if !has("package.json") => Some("package.json"),
        Runtime::Python
            if !["requirements.txt", "pyproject.toml", "Pipfile", "setup.py"].iter().any(|f| has(f))
                && !has_extension(dir, "py") =>
        {
            Some("requirements.txt, pyproject.toml, Pipfile or .py file")
        }
        Runtime::Go if !(has("go.mod") || (custom_build && has_extension(dir, "go"))) => Some("go.mod"),
        Runtime::Rust if !has("Cargo.toml") => Some("Cargo.toml"),
        Runtime::Ruby if !has("Gemfile") => Some("Gemfile"),
        _ => None,
    };
    let Some(what) = missing else { return Ok(()) };
    let hint = match detect::detect(dir) {
        Some(Runtime::Docker) => " (it has a Dockerfile: set the runtime to 'docker' or 'auto')".to_string(),
        Some(other) if other != runtime => {
            format!(" (it looks like a {} project: set the runtime to '{other}' or 'auto')", project_label(other))
        }
        _ => String::new(),
    };
    Err(Error::invalid(format!("runtime '{runtime}' was selected but the root directory has no {what}{hint}")))
}

fn project_label(r: Runtime) -> &'static str {
    match r {
        Runtime::Node => "Node.js",
        Runtime::Python => "Python",
        Runtime::Go => "Go",
        Runtime::Rust => "Rust",
        Runtime::Ruby => "Ruby",
        Runtime::Static => "static site",
        Runtime::Docker | Runtime::Image | Runtime::Auto => "Docker",
    }
}

fn has_extension(dir: &Path, ext: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == ext) && e.path().is_file()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Build-time environment

/// Build-arg names that are safe to expose (valid environment variable and
/// BuildKit secret names) and that do not clobber variables every build step
/// (or the docker CLI itself) relies on.
pub(crate) fn usable_build_arg(key: &str) -> bool {
    let mut chars = key.chars();
    let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && key.len() <= 256;
    valid && !is_reserved_build_arg(key)
}

fn is_reserved_build_arg(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "PATH",
        "HOME",
        "PWD",
        "OLDPWD",
        "HOSTNAME",
        "SHELL",
        "USER",
        "LOGNAME",
        "TERM",
        "SHLVL",
        "TMPDIR",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "SSH_AUTH_SOCK",
    ];
    const PREFIXES: &[&str] = &["DOCKER_", "BUILDKIT_", "BUILDX_", "COMPOSE_", "XDG_", RESERVED_PREFIX];
    EXACT.contains(&key) || PREFIXES.iter().any(|p| key.starts_with(p))
}

/// The build-time variables a generated Dockerfile exposes: usable names
/// only, first occurrence wins, in order.
pub(crate) fn build_env_keys(keys: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for k in keys {
        if usable_build_arg(k) && !out.contains(&k.as_str()) {
            out.push(k);
        }
    }
    out
}

/// BuildKit secret id of a build-time variable. Usable names are valid ids
/// as they are; anything else is sanitized so it can never break the
/// `--secret` / `--mount` CSV syntax.
pub(crate) fn secret_id(key: &str) -> String {
    key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

// ---------------------------------------------------------------------------
// Dockerfile writer

/// `RUN ` plus build flags, one per continuation line for readability.
fn run_prefix(flags: &[String]) -> String {
    if flags.is_empty() { "RUN ".to_string() } else { format!("RUN {} \\\n    ", flags.join(" \\\n    ")) }
}

/// Can `cmd` be written as a shell-form instruction on one Dockerfile line?
fn shell_form_ok(cmd: &str) -> bool {
    !cmd.contains('\n') && !cmd.contains('\r') && !cmd.contains("<<") && !cmd.trim_end().ends_with('\\')
}

fn json(v: &[&str]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

struct Df {
    lines: Vec<String>,
    /// `--mount=type=secret,…` flags exposing the build-time env.
    env_mounts: Vec<String>,
}

impl Df {
    fn new(header: &str, opts: &DockerfileOptions) -> Self {
        let env_mounts = build_env_keys(&opts.build_arg_keys)
            .into_iter()
            .map(|k| format!("--mount=type=secret,id={},env={k}", secret_id(k)))
            .collect();
        let mut d = Df { lines: Vec::new(), env_mounts };
        d.line(format!("# Generated by Ferry: {header}."));
        d.line("# Commit your own Dockerfile to customize the build.");
        d
    }

    fn line(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.lines.push(String::new());
        }
    }

    /// Declare the env digest arg, right before the first step that sees the
    /// build-time env (so a changed variable invalidates the cache from there).
    fn declare_build_env(&mut self) {
        if !self.env_mounts.is_empty() {
            self.line(format!("ARG {ENV_DIGEST_ARG}"));
        }
    }

    /// `RUN` a (possibly user-supplied) command, with optional `--mount` flags.
    /// For system steps that must not see the service's env.
    fn run(&mut self, flags: &[String], cmd: &str) {
        if shell_form_ok(cmd) {
            self.line(format!("{}{}", run_prefix(flags), cmd.trim()));
        } else {
            self.line(format!("{}{}", run_prefix(flags), json(&["/bin/sh", "-c", cmd])));
        }
    }

    /// `RUN` a step that executes project code (installs, builds): it also
    /// gets the build-time env through secret mounts.
    fn run_env(&mut self, flags: &[String], cmd: &str) {
        let flags = self.with_env(flags);
        self.run(&flags, cmd);
    }

    fn with_env(&self, flags: &[String]) -> Vec<String> {
        flags.iter().chain(&self.env_mounts).cloned().collect()
    }

    /// `RUN` a sequence of project steps (with the build-time env); `user`
    /// marks user-supplied commands, which are isolated in a subshell so
    /// their own `;`/`||` can't change the chain. Multi-line commands (or
    /// ones with `#` comments) become a `set -e` script.
    fn run_steps_env(&mut self, flags: &[String], steps: &[(bool, String)]) {
        let flags = self.with_env(flags);
        let chainable = steps.iter().all(|(user, s)| shell_form_ok(s) && !(*user && s.contains('#')));
        if chainable {
            let chain: Vec<String> =
                steps.iter().map(|(user, s)| if *user { format!("( {} )", s.trim()) } else { s.clone() }).collect();
            self.run(&flags, &chain.join(" && "));
        } else {
            let mut script = String::from("set -e\n");
            for (_, s) in steps {
                script.push_str(s.trim_end());
                script.push('\n');
            }
            self.line(format!("{}{}", run_prefix(&flags), json(&["/bin/sh", "-c", &script])));
        }
    }

    /// Start command run by a shell (so `$PORT` and friends expand). Written
    /// as `["/bin/sh","-c",…]`: exactly what Docker turns shell-form CMD into,
    /// but safe for any characters and free of BuildKit's JSONArgs warning.
    fn cmd_shell(&mut self, cmd: &str) {
        self.line(format!("CMD {}", json(&["/bin/sh", "-c", cmd.trim()])));
    }

    fn cmd_exec(&mut self, argv: &[&str]) {
        self.line(format!("CMD {}", json(argv)));
    }

    fn finish(mut self) -> String {
        while self.lines.last().is_some_and(|l| l.is_empty()) {
            self.lines.pop();
        }
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn exists(dir: &Path, name: &str) -> bool {
    dir.join(name).exists()
}

/// `apt-get install` of Debian packages in one cleaned-up layer.
fn apt_install(pkgs: &[&str]) -> String {
    format!(
        "apt-get update && apt-get install -y --no-install-recommends {} && rm -rf /var/lib/apt/lists/*",
        pkgs.join(" ")
    )
}

/// How the image starts.
enum StartCmd {
    /// Run by a shell (so `$PORT` and friends expand).
    Shell(String),
    Exec(Vec<String>),
    /// No command (cron jobs get their command at run time).
    None,
}

/// The start command of a generated image, and where it comes from (the
/// build log says both: a default nobody chose is the first thing to look at
/// when an instance doesn't stay up).
struct Start {
    cmd: StartCmd,
    source: String,
}

impl Start {
    fn shell(cmd: impl Into<String>, source: impl Into<String>) -> Self {
        Start { cmd: StartCmd::Shell(cmd.into()), source: source.into() }
    }

    fn exec(argv: Vec<String>, source: impl Into<String>) -> Self {
        Start { cmd: StartCmd::Exec(argv), source: source.into() }
    }

    fn none() -> Self {
        Start { cmd: StartCmd::None, source: String::new() }
    }
}

/// The start command the user chose: the service's own, else the entry of
/// its Procfile.
fn chosen_start(dir: &Path, opts: &DockerfileOptions) -> Option<Start> {
    if let Some(s) = nonempty(&opts.start_command) {
        return Some(Start::shell(s, "the service's start command"));
    }
    detect::procfile_command(dir, opts.service_type).map(|p| Start::shell(p, "the Procfile"))
}

/// Write the `CMD`; returns what the build log says about it.
fn emit_start(df: &mut Df, start: Start) -> Option<StartCommand> {
    let command = match start.cmd {
        StartCmd::Shell(c) => {
            df.cmd_shell(&c);
            c.trim().to_string()
        }
        StartCmd::Exec(argv) => {
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            df.cmd_exec(&refs);
            argv.join(" ")
        }
        StartCmd::None => return None,
    };
    Some(StartCommand { command, source: start.source })
}

fn missing_start(what: &str, hint: &str, opts: &DockerfileOptions) -> Result<Start> {
    if opts.service_type == Some(ServiceType::CronJob) {
        return Ok(Start::none());
    }
    let entry = match opts.service_type {
        Some(ServiceType::BackgroundWorker) => "worker",
        _ => "web",
    };
    Err(Error::invalid(format!(
        "cannot determine how to start this {what} app: set a start command, add a Procfile with a `{entry}:` entry{hint}"
    )))
}

// ---------------------------------------------------------------------------
// Node.js

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pm {
    Npm,
    Yarn,
    YarnBerry,
    Pnpm,
    Bun,
}

impl Pm {
    fn name(self) -> &'static str {
        match self {
            Pm::Npm => "npm",
            Pm::Yarn | Pm::YarnBerry => "yarn",
            Pm::Pnpm => "pnpm",
            Pm::Bun => "bun",
        }
    }

    fn run_script(self, script: &str) -> String {
        format!("{} run {script}", self.name())
    }
}

struct NodePlan {
    pm: Pm,
    base_image: String,
    install: String,
    manifests: Vec<&'static str>,
    copy_yarn_dir: bool,
    copy_all_first: bool,
    has_build_script: bool,
    has_start_script: bool,
    main: Option<String>,
}

impl NodePlan {
    /// A build command without package.json: run it in a plain Node image.
    fn bare() -> NodePlan {
        NodePlan {
            pm: Pm::Npm,
            base_image: format!("node:{}-alpine", detect::DEFAULT_NODE_MAJOR),
            install: String::new(),
            manifests: Vec::new(),
            copy_yarn_dir: false,
            copy_all_first: true,
            has_build_script: false,
            has_start_script: false,
            main: None,
        }
    }
}

const NODE_MANIFESTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pnpmfile.cjs",
    "pnpm-workspace.yaml",
    "bunfig.toml",
];

fn read_package_json(dir: &Path) -> Result<Option<Value>> {
    match read_text(dir, "package.json") {
        None => Ok(None),
        Some(s) => serde_json::from_str::<Value>(&s)
            .map(Some)
            .map_err(|e| Error::invalid(format!("package.json is not valid JSON: {e}"))),
    }
}

fn script<'a>(pkg: &'a Value, name: &str) -> Option<&'a str> {
    pkg.get("scripts")?.get(name)?.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn node_plan(dir: &Path) -> Result<NodePlan> {
    let pkg = read_package_json(dir)?;
    let empty = Value::Object(Default::default());
    let p = pkg.as_ref().unwrap_or(&empty);
    let has = |f: &str| dir.join(f).is_file();

    let field = p.get("packageManager").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let field_major = |name: &str| -> Option<u32> {
        let v = field.strip_prefix(name)?.strip_prefix('@')?;
        v.split('.').next()?.parse().ok()
    };
    let berry_markers = has(".yarnrc.yml")
        || read_text(dir, "yarn.lock").is_some_and(|l| l.contains("__metadata:"))
        || field_major("yarn").is_some_and(|m| m >= 2);
    let pm = if field.starts_with("pnpm@") {
        Pm::Pnpm
    } else if field.starts_with("yarn@") {
        if berry_markers { Pm::YarnBerry } else { Pm::Yarn }
    } else if field.starts_with("bun@") {
        Pm::Bun
    } else if field.starts_with("npm@") {
        Pm::Npm
    } else if has("bun.lockb") || has("bun.lock") {
        Pm::Bun
    } else if has("pnpm-lock.yaml") {
        Pm::Pnpm
    } else if has("yarn.lock") {
        if berry_markers { Pm::YarnBerry } else { Pm::Yarn }
    } else {
        Pm::Npm
    };

    let install = match pm {
        Pm::Npm if has("package-lock.json") || has("npm-shrinkwrap.json") => {
            "npm ci --include=dev --no-audit --no-fund"
        }
        Pm::Npm => "npm install --include=dev --no-audit --no-fund",
        Pm::Yarn if has("yarn.lock") => "yarn install --frozen-lockfile --production=false",
        Pm::Yarn => "yarn install --production=false",
        Pm::YarnBerry if has("yarn.lock") => "yarn install --immutable",
        Pm::YarnBerry => "yarn install",
        Pm::Pnpm if has("pnpm-lock.yaml") => "pnpm install --frozen-lockfile",
        Pm::Pnpm => "pnpm install",
        Pm::Bun => "bun install",
    }
    .to_string();

    let major = node_major_for(p.get("engines").and_then(|e| e.get("node")).and_then(Value::as_str));
    let base_image = if pm == Pm::Bun { "oven/bun:1-alpine".to_string() } else { format!("node:{major}-alpine") };

    // Installing from the manifests alone breaks when install needs other
    // files: workspaces, lifecycle scripts, local dependencies.
    let lifecycle = ["preinstall", "install", "postinstall", "prepare"].iter().any(|s| script(p, s).is_some());
    let local_deps = ["dependencies", "devDependencies", "optionalDependencies"].iter().any(|k| {
        p.get(*k).and_then(Value::as_object).is_some_and(|deps| {
            deps.values()
                .filter_map(Value::as_str)
                .any(|v| ["file:", "link:", "portal:", "workspace:"].iter().any(|pre| v.trim_start().starts_with(pre)))
        })
    });
    let copy_all_first =
        pkg.is_none() || p.get("workspaces").is_some() || has("pnpm-workspace.yaml") || lifecycle || local_deps;

    let manifests: Vec<&'static str> = NODE_MANIFESTS.iter().copied().filter(|f| has(f)).collect();
    let copy_yarn_dir = pm == Pm::YarnBerry && dir.join(".yarn").is_dir();
    let main = p.get("main").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);

    Ok(NodePlan {
        pm,
        base_image,
        install,
        manifests,
        copy_yarn_dir,
        copy_all_first,
        has_build_script: script(p, "build").is_some(),
        has_start_script: script(p, "start").is_some(),
        main,
    })
}

/// FROM … through the build step, shared by Node apps and node-built static
/// sites. `system` is an extra system step (e.g. installing Hugo).
fn node_build_stage(df: &mut Df, plan: &NodePlan, stage: Option<&str>, build: Option<&str>, system: Option<&str>) {
    match stage {
        Some(name) => df.line(format!("FROM {} AS {name}", plan.base_image)),
        None => df.line(format!("FROM {}", plan.base_image)),
    }
    if let Some(s) = system {
        df.run(&[], s);
    }
    df.line("WORKDIR /app");
    if matches!(plan.pm, Pm::Yarn | Pm::YarnBerry | Pm::Pnpm) {
        df.line("ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0");
        df.run(&[], "(corepack --version >/dev/null 2>&1 || npm install -g corepack) && corepack enable");
    }
    df.declare_build_env();
    df.blank();
    if plan.install.is_empty() {
        df.line("COPY . .");
    } else if plan.copy_all_first || plan.manifests.is_empty() {
        df.line("COPY . .");
        df.run_env(&[], &plan.install);
    } else {
        df.line(format!("COPY {} ./", plan.manifests.join(" ")));
        if plan.copy_yarn_dir {
            df.line("COPY .yarn ./.yarn");
        }
        df.run_env(&[], &plan.install);
        df.blank();
        df.line("COPY . .");
    }
    // After install, so devDependencies needed by the build are installed.
    df.line("ENV NODE_ENV=production");
    if let Some(b) = build {
        df.run_env(&[], b);
    }
}

fn node(dir: &Path, opts: &DockerfileOptions) -> Result<Generated> {
    let plan = node_plan(dir)?;
    let build = nonempty(&opts.build_command)
        .map(str::to_string)
        .or_else(|| plan.has_build_script.then(|| plan.pm.run_script("build")));
    // The file a package.json without a "start" script is run from: its
    // "main" (which a build may produce), else a conventional file.
    let main = plan.main.clone().filter(|m| build.is_some() || dir.join(m).is_file());
    let start = if let Some(chosen) = chosen_start(dir, opts) {
        chosen
    } else if plan.has_start_script {
        let argv = match plan.pm {
            Pm::YarnBerry => vec!["yarn".into(), "start".into()],
            Pm::Bun => vec!["bun".into(), "run".into(), "start".into()],
            _ => vec!["npm".into(), "start".into()],
        };
        Start::exec(argv, "the \"start\" script of package.json")
    } else if let Some((entry, source)) =
        main.map(|m| (m, "package.json has no \"start\" script: its \"main\" file is run".to_string())).or_else(|| {
            let file = ["server.js", "index.js", "app.js"].iter().find(|f| dir.join(f).is_file())?;
            Some((file.to_string(), format!("package.json has no \"start\" script: {file} is run")))
        })
    {
        let argv = match plan.pm {
            Pm::YarnBerry => vec!["yarn".into(), "node".into(), entry],
            Pm::Bun => vec!["bun".into(), entry],
            _ => vec!["node".into(), entry],
        };
        Start::exec(argv, source)
    } else {
        missing_start("Node.js", ", a \"start\" script to package.json, or a server.js / index.js file", opts)?
    };

    let mut df = Df::new(&format!("Node.js app ({})", plan.pm.name()), opts);
    node_build_stage(&mut df, &plan, None, build.as_deref(), None);
    df.blank();
    let start = emit_start(&mut df, start);
    Ok((df.finish(), start))
}

// ---------------------------------------------------------------------------
// Python

/// How a Python project's dependencies get installed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PyInstall {
    /// `pip install -r requirements.txt`; `needs_tree` when it references
    /// other files of the project.
    Requirements {
        needs_tree: bool,
    },
    /// `uv.lock`: install the locked dependencies (plus the project itself
    /// when it is a package).
    Uv {
        with_project: bool,
    },
    /// `poetry.lock` / `[tool.poetry]`.
    Poetry {
        with_project: bool,
    },
    /// A packaged project without a lock file: `pip install .`.
    Package,
    /// A PEP 621 app that is not a package (`uv init` layout): its
    /// `[project] dependencies` only.
    ProjectDeps {
        needs_tree: bool,
    },
    Pipenv {
        locked: bool,
    },
    Nothing,
}

/// Normalized distribution name as a Python import name.
fn import_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['-', '.'], "_")
}

/// Is the project meant to be installed as a package? It must declare a
/// build system, not opt out of package mode, and have something a build
/// backend can find (explicit configuration, or a package / module named
/// after the project) — otherwise building it fails (flat layouts with
/// several modules, Poetry apps whose name matches no directory).
fn python_packaged(dir: &Path, py: &detect::PyProject) -> bool {
    if !py.build_system || py.not_a_package {
        return false;
    }
    if py.explicit_packages {
        return true;
    }
    let Some(name) = py.name.as_deref().map(import_name) else { return false };
    [dir.to_path_buf(), dir.join("src")]
        .iter()
        .any(|base| base.join(&name).is_dir() || base.join(format!("{name}.py")).is_file())
}

fn python_install(dir: &Path) -> PyInstall {
    let has = |f: &str| dir.join(f).is_file();
    if has("requirements.txt") {
        let reqs = read_text(dir, "requirements.txt").unwrap_or_default();
        // References to other files (-r, -c, -e ., local paths) need the full tree.
        let needs_tree = reqs.lines().map(str::trim).any(|l| {
            l.starts_with("-r")
                || l.starts_with("-c")
                || l.starts_with("-e")
                || l.starts_with("--requirement")
                || l.starts_with("--constraint")
                || l.starts_with("--editable")
                || l.starts_with('.')
                || l.starts_with("file:")
        });
        return PyInstall::Requirements { needs_tree };
    }
    if has("pyproject.toml") {
        let py = detect::parse_pyproject(&read_text(dir, "pyproject.toml").unwrap_or_default());
        let packaged = python_packaged(dir, &py);
        if has("uv.lock") {
            return PyInstall::Uv { with_project: packaged };
        }
        if has("poetry.lock") || py.poetry {
            return PyInstall::Poetry { with_project: packaged };
        }
        if packaged {
            return PyInstall::Package;
        }
        if py.project {
            return PyInstall::ProjectDeps { needs_tree: py.dynamic };
        }
        // A pyproject.toml holding tool settings only: look further.
    }
    if has("Pipfile") {
        return PyInstall::Pipenv { locked: has("Pipfile.lock") };
    }
    PyInstall::Nothing
}

/// Debian packages that database drivers need: `psycopg2` / `psycopg[c]` /
/// `mysqlclient` build against the client libraries, pure-Python `psycopg`
/// loads `libpq` at run time. The `-binary` variants bundle their own.
fn python_system_packages(dir: &Path) -> Vec<&'static str> {
    let mut text = String::new();
    let mut files: Vec<String> =
        ["pyproject.toml", "Pipfile", "Pipfile.lock", "poetry.lock", "uv.lock", "setup.py", "setup.cfg"]
            .iter()
            .map(|f| f.to_string())
            .collect();
    let list = |d: &Path, prefix: &str| -> Vec<String> {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|n| n.ends_with(".txt") && (prefix.is_empty() || n.starts_with("requirements")))
                    .collect()
            })
            .unwrap_or_default()
    };
    files.extend(list(dir, "requirements"));
    files.extend(list(&dir.join("requirements"), "").into_iter().map(|n| format!("requirements/{n}")));
    for f in files {
        if let Some(t) = read_text(dir, &f) {
            text.push_str(&t.to_ascii_lowercase());
            text.push('\n');
        }
    }
    let mut pkgs: BTreeSet<&'static str> = BTreeSet::new();
    for (i, _) in text.match_indices("psycopg") {
        let rest = &text[i + "psycopg".len()..];
        let binary = ["2-binary", "2_binary", "-binary", "_binary", "[binary"].iter().any(|b| rest.starts_with(b));
        let pool = rest.starts_with("-pool") || rest.starts_with("_pool");
        if binary || pool {
            continue;
        }
        if rest.starts_with('2') || rest.starts_with("-c") || rest.starts_with("_c") || rest.starts_with("[c") {
            pkgs.extend(["build-essential", "libpq-dev"]);
        } else {
            pkgs.insert("libpq5");
        }
    }
    if text.contains("mysqlclient") {
        pkgs.extend(["build-essential", "default-libmysqlclient-dev", "pkg-config"]);
    }
    if pkgs.contains("libpq-dev") {
        pkgs.remove("libpq5");
    }
    pkgs.into_iter().collect()
}

/// FROM … through the dependency install and `COPY . .` (the build command
/// comes next), shared by Python apps and Python-built static sites.
fn python_stage(df: &mut Df, dir: &Path, stage: Option<&str>) {
    let version = detect::python_version(dir).unwrap_or_else(|| DEFAULT_PYTHON.to_string());
    match stage {
        Some(name) => df.line(format!("FROM python:{version}-slim AS {name}")),
        None => df.line(format!("FROM python:{version}-slim")),
    }
    df.line("ENV PYTHONUNBUFFERED=1 \\");
    df.line("    PYTHONDONTWRITEBYTECODE=1 \\");
    df.line("    PIP_DISABLE_PIP_VERSION_CHECK=1 \\");
    df.line("    PIP_ROOT_USER_ACTION=ignore");
    let pkgs = python_system_packages(dir);
    if !pkgs.is_empty() {
        df.run(&[], &apt_install(&pkgs));
    }
    df.line("WORKDIR /app");
    df.declare_build_env();
    df.blank();

    let uv_cache = ["--mount=type=cache,target=/root/.cache/uv".to_string()];
    match python_install(dir) {
        PyInstall::Requirements { needs_tree: true } => {
            df.line("COPY . .");
            df.run_env(&[], "pip install --no-cache-dir -r requirements.txt");
        }
        PyInstall::Requirements { needs_tree: false } => {
            df.line("COPY requirements.txt ./");
            df.run_env(&[], "pip install --no-cache-dir -r requirements.txt");
            df.blank();
            df.line("COPY . .");
        }
        PyInstall::Uv { with_project } => {
            df.run_env(&[], "pip install --no-cache-dir uv");
            df.line("COPY . .");
            let project = if with_project { "" } else { " --no-emit-project" };
            df.run_env(
                &uv_cache,
                &format!(
                    "uv export --frozen --no-dev{project} --no-hashes --output-file /tmp/ferry-requirements.txt \
                     && UV_LINK_MODE=copy uv pip install --system -r /tmp/ferry-requirements.txt"
                ),
            );
        }
        PyInstall::Poetry { with_project } => {
            df.run_env(&[], "pip install --no-cache-dir poetry");
            df.line("COPY . .");
            let root = if with_project { "" } else { " --no-root" };
            df.run_env(
                &[],
                &format!("POETRY_VIRTUALENVS_CREATE=false poetry install --only main --no-interaction --no-ansi{root}"),
            );
        }
        PyInstall::Package => {
            df.line("COPY . .");
            df.run_env(&[], "pip install --no-cache-dir .");
        }
        PyInstall::ProjectDeps { needs_tree } => {
            df.run_env(&[], "pip install --no-cache-dir uv");
            df.line(if needs_tree { "COPY . ." } else { "COPY pyproject.toml ./" });
            df.run_env(&uv_cache, "UV_LINK_MODE=copy uv pip install --system -r pyproject.toml");
            if !needs_tree {
                df.blank();
                df.line("COPY . .");
            }
        }
        PyInstall::Pipenv { locked } => {
            df.run_env(&[], "pip install --no-cache-dir pipenv");
            df.line(if locked { "COPY Pipfile Pipfile.lock ./" } else { "COPY Pipfile ./" });
            df.run_env(
                &[],
                if locked { "pipenv install --system --deploy" } else { "pipenv install --system --skip-lock" },
            );
            df.blank();
            df.line("COPY . .");
        }
        PyInstall::Nothing => df.line("COPY . ."),
    }
}

fn python(dir: &Path, opts: &DockerfileOptions) -> Result<Generated> {
    let mut df = Df::new("Python app", opts);
    python_stage(&mut df, dir, None);
    if let Some(b) = nonempty(&opts.build_command) {
        df.run_env(&[], b);
    }
    df.blank();

    let start = if let Some(chosen) = chosen_start(dir, opts) {
        chosen
    } else if let Some(f) = ["main.py", "app.py"].iter().find(|f| dir.join(f).is_file()) {
        Start::exec(vec!["python".into(), f.to_string()], format!("the Python default: {f} is run"))
    } else {
        missing_start("Python", ", or a main.py / app.py file", opts)?
    };
    let start = emit_start(&mut df, start);
    Ok((df.finish(), start))
}

// ---------------------------------------------------------------------------
// Go

/// Package to build: `.` when the root is `package main`, else the single
/// main package under `cmd/`. Anything else needs a build command.
fn go_main_package(dir: &Path) -> Result<String> {
    if detect::go_dir_is_main(dir) {
        return Ok(".".to_string());
    }
    let mut cmds: Vec<String> = std::fs::read_dir(dir.join("cmd"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir() && detect::go_dir_is_main(&e.path()))
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
                .collect()
        })
        .unwrap_or_default();
    cmds.sort();
    match cmds.as_slice() {
        [only] => Ok(format!("./cmd/{only}")),
        [] => Err(Error::invalid(
            "cannot determine which Go package to build: the root directory is not `package main` and \
             there is no main package under cmd/<name>; set a build command such as \
             `go build -o /out/app ./path/to/main` (binaries in /out are installed in /usr/local/bin)",
        )),
        many => Err(Error::invalid(format!(
            "cannot determine which Go package to build: several commands under cmd/ ({}); set a build \
             command such as `go build -o /out/app ./cmd/{}` (binaries in /out are installed in /usr/local/bin)",
            many.join(", "),
            many[0]
        ))),
    }
}

/// Go cache mounts: module cache and build cache.
fn go_caches() -> [String; 2] {
    ["--mount=type=cache,target=/go/pkg/mod".to_string(), "--mount=type=cache,target=/root/.cache/go-build".to_string()]
}

/// FROM … through `COPY . .` of a Go build stage (module download cached
/// separately when possible).
fn go_source_stage(df: &mut Df, dir: &Path) {
    let version = detect::go_version(dir).unwrap_or_else(|| DEFAULT_GO.to_string());
    let gomod = read_text(dir, "go.mod").unwrap_or_default();
    let local_replace = gomod.lines().any(|l| l.contains("=> ./") || l.contains("=> ../") || l.contains("=> /"));
    let vendored = dir.join("vendor").is_dir();
    df.line(format!("FROM golang:{version}-alpine AS build"));
    df.line("WORKDIR /src");
    df.declare_build_env();
    df.blank();
    if exists(dir, "go.mod") && !local_replace && !vendored {
        df.line(if exists(dir, "go.sum") { "COPY go.mod go.sum ./" } else { "COPY go.mod ./" });
        let [mod_cache, _] = go_caches();
        df.run_env(std::slice::from_ref(&mod_cache), "go mod download");
        df.blank();
    }
    df.line("COPY . .");
}

fn go(dir: &Path, opts: &DockerfileOptions) -> Result<Generated> {
    let steps: Vec<(bool, String)> = match nonempty(&opts.build_command) {
        Some(b) => vec![(false, "mkdir -p /out".into()), (true, b.to_string())],
        None => vec![
            (false, "mkdir -p /out".into()),
            (
                false,
                format!("CGO_ENABLED=0 go build -trimpath -ldflags=\"-s -w\" -o /out/app {}", go_main_package(dir)?),
            ),
        ],
    };
    let mut df = Df::new("Go app", opts);
    go_source_stage(&mut df, dir);
    df.run_steps_env(&go_caches(), &steps);
    df.blank();
    df.line("FROM alpine:3.20");
    df.run(&[], "apk add --no-cache ca-certificates tzdata");
    df.line("WORKDIR /app");
    // The source tree stays available for runtime assets (templates, static
    // files) and Render-style start commands such as `./app`.
    df.line("COPY --from=build /src/ /app/");
    df.line("COPY --from=build /out/ /usr/local/bin/");
    df.blank();
    let start = chosen_start(dir, opts)
        .unwrap_or_else(|| Start::exec(vec!["/usr/local/bin/app".into()], "the Go default: the built binary is run"));
    let start = emit_start(&mut df, start);
    Ok((df.finish(), start))
}

// ---------------------------------------------------------------------------
// Rust

fn rust(dir: &Path, opts: &DockerfileOptions) -> Result<Generated> {
    let info: CargoInfo = detect::parse_cargo_toml(&read_text(dir, "Cargo.toml").unwrap_or_default());
    let bin = detect::rust_binary(dir, &info);
    if let Some(b) = &bin
        && !b.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::invalid(format!("unsupported Rust binary name '{b}'")));
    }
    let chosen = chosen_start(dir, opts);
    if bin.is_none() && chosen.is_none() && opts.service_type != Some(ServiceType::CronJob) {
        return Err(Error::invalid(
            "cannot determine which binary to run (Cargo workspace without a root package): set a start command \
             naming the binary; release binaries are installed in /usr/local/bin",
        ));
    }
    let needs_openssl = read_text(dir, "Cargo.lock").is_some_and(|l| l.contains("name = \"openssl-sys\""));
    let cache_id: String = info
        .package_name
        .as_deref()
        .or(bin.as_deref())
        .unwrap_or("workspace")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();

    let mut df = Df::new("Rust app", opts);
    df.line("FROM rust:1-slim-bookworm AS build");
    if needs_openssl {
        df.run(&[], &apt_install(&["pkg-config", "libssl-dev"]));
    }
    df.line("WORKDIR /src");
    df.declare_build_env();
    df.blank();
    df.line("COPY . .");
    let mut steps: Vec<(bool, String)> = vec![
        // Sources must look newer than outputs in the cached target dir.
        (false, "find . -path ./target -prune -o -type f -exec touch {} +".into()),
    ];
    match nonempty(&opts.build_command) {
        Some(b) => steps.push((true, b.to_string())),
        None => steps.push((false, "cargo build --release".into())),
    }
    steps.push((false, "mkdir -p /out".into()));
    match &bin {
        Some(b) => steps.push((false, format!("cp target/release/{b} /out/"))),
        None => steps.push((false, "find target/release -maxdepth 1 -type f -perm -u+x -exec cp {} /out/ \\;".into())),
    }
    df.run_steps_env(
        &[
            "--mount=type=cache,target=/usr/local/cargo/registry,sharing=locked".to_string(),
            "--mount=type=cache,target=/usr/local/cargo/git,sharing=locked".to_string(),
            format!("--mount=type=cache,target=/src/target,id=ferry-cargo-target-{cache_id},sharing=locked"),
        ],
        &steps,
    );
    df.blank();
    df.line("FROM debian:bookworm-slim");
    let runtime_pkgs: &[&str] = if needs_openssl { &["ca-certificates", "libssl3"] } else { &["ca-certificates"] };
    df.run(&[], &apt_install(runtime_pkgs));
    df.line("WORKDIR /app");
    df.line("COPY --from=build /src/ /app/");
    df.line("COPY --from=build /out/ /usr/local/bin/");
    df.blank();
    let start = match (chosen, bin) {
        (Some(chosen), _) => chosen,
        (None, Some(b)) => {
            Start::exec(vec![format!("/usr/local/bin/{b}")], "the Rust default: the package's binary is run")
        }
        (None, None) => Start::none(),
    };
    let start = emit_start(&mut df, start);
    Ok((df.finish(), start))
}

// ---------------------------------------------------------------------------
// Ruby

/// FROM … through `bundle install` and `COPY . .`, shared by Ruby apps and
/// Ruby-built static sites (Jekyll).
fn ruby_stage(df: &mut Df, dir: &Path, stage: Option<&str>) {
    let version = detect::ruby_version(dir).unwrap_or_else(|| DEFAULT_RUBY.to_string());
    let gemfile = read_text(dir, "Gemfile").unwrap_or_default();
    let lock = read_text(dir, "Gemfile.lock").unwrap_or_default();
    let needs_tree = gemfile
        .lines()
        .map(str::trim)
        .any(|l| !l.starts_with('#') && (l.starts_with("gemspec") || l.contains("path:") || l.contains(":path")));
    match stage {
        Some(name) => df.line(format!("FROM ruby:{version}-slim AS {name}")),
        None => df.line(format!("FROM ruby:{version}-slim")),
    }
    // libpq-dev: the `pg` gem (Rails' default Postgres driver) compiles
    // against it, like in the Dockerfile Rails generates.
    let mut pkgs = vec!["build-essential", "git", "libpq-dev", "libyaml-dev", "pkg-config"];
    if gemfile.contains("mysql2") || lock.contains("mysql2") {
        pkgs.push("default-libmysqlclient-dev");
    }
    df.run(&[], &apt_install(&pkgs));
    df.line("WORKDIR /app");
    df.declare_build_env();
    df.blank();
    if needs_tree || !exists(dir, "Gemfile") {
        df.line("COPY . .");
        df.run_env(&[], "bundle install");
    } else {
        df.line(if exists(dir, "Gemfile.lock") { "COPY Gemfile Gemfile.lock ./" } else { "COPY Gemfile ./" });
        df.run_env(&[], "bundle install");
        df.blank();
        df.line("COPY . .");
    }
}

fn ruby(dir: &Path, opts: &DockerfileOptions) -> Result<Generated> {
    let mut df = Df::new("Ruby app", opts);
    ruby_stage(&mut df, dir, None);
    if let Some(b) = nonempty(&opts.build_command) {
        df.run_env(&[], b);
    }
    df.blank();
    let start = if let Some(chosen) = chosen_start(dir, opts) {
        chosen
    } else if exists(dir, "config.ru") {
        Start::shell(
            "bundle exec rackup -o 0.0.0.0 -p ${PORT:-10000}",
            "the Ruby default for a Rack app: config.ru is served",
        )
    } else {
        missing_start("Ruby", ", or a config.ru for Rack apps", opts)?
    };
    let start = emit_start(&mut df, start);
    Ok((df.finish(), start))
}

// ---------------------------------------------------------------------------
// Static sites

/// Build outputs looked for when no publish directory is set: common JS
/// bundler outputs, then Jekyll's `_site` and MkDocs' `site`.
const PUBLISH_CANDIDATES: &[&str] = &["dist", "build", "out", "public", "_site", "site"];

fn nginx_config_lines() -> Vec<&'static str> {
    vec![
        "server {",
        "    listen 80;",
        "    server_name _;",
        "    root /usr/share/nginx/html;",
        "    index index.html index.htm;",
        "    absolute_redirect off;",
        "    gzip on;",
        "    gzip_vary on;",
        "    gzip_min_length 256;",
        "    gzip_types text/plain text/css text/xml text/javascript application/javascript application/json application/xml application/manifest+json image/svg+xml;",
        "    location / {",
        "        try_files $uri $uri/ $uri.html =404;",
        "    }",
        // Dotfiles (.git*, .env*, .npmrc, .ferryignore...) are never served;
        // /.well-known/ stays public (security.txt, app links, ACME).
        "    location ~ /\\.(?!well-known(/|$)) {",
        "        return 404;",
        "    }",
        "    error_page 404 /404.html;",
        "}",
    ]
}

/// What a static site serves, from its publish directory setting.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Publish {
    /// Not set: a build output (dist, build, out, ...) when there is one,
    /// else the root directory.
    Auto,
    /// Explicitly the root directory (`.`): served as-is, even when build
    /// output directories exist.
    Root,
    /// This directory (normalized, shell/Dockerfile-safe).
    Dir(String),
}

fn publish_dir(opts: &DockerfileOptions) -> Result<Publish> {
    let Some(raw) = nonempty(&opts.publish_dir) else { return Ok(Publish::Auto) };
    let rel = crate::fsutil::normalize_relative("publish directory", raw).map_err(Error::invalid)?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        return Ok(Publish::Root);
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '@' | '+')) {
        return Err(Error::invalid(format!(
            "publish directory '{raw}' contains unsupported characters (use letters, digits, '.', '_', '-', '/')"
        )));
    }
    Ok(Publish::Dir(s))
}

/// Ferry's own build files and the git metadata are never served.
fn strip_ferry_files(dir: &str) -> String {
    format!("rm -rf {dir}/Dockerfile.ferry* {dir}/.git")
}

fn nginx_stage(df: &mut Df) {
    df.line("FROM nginx:alpine");
    let mut run = format!("RUN rm -rf {NGINX_HTML}/* \\\n && printf '%s\\n' \\\n");
    for l in nginx_config_lines() {
        run.push_str(&format!("    '{l}' \\\n"));
    }
    run.push_str("    > /etc/nginx/conf.d/default.conf");
    df.line(run);
}

/// The toolchain a static site's build command runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaticBuild {
    /// package.json: Node with the project's package manager.
    Node,
    /// requirements.txt / pyproject.toml / Pipfile (MkDocs, Pelican…).
    Python,
    /// Gemfile (Jekyll, Middleman…).
    Ruby,
    /// Hugo (config file, or a build command running `hugo`).
    Hugo,
    /// go.mod: a Go program generating the site.
    Go,
    /// Nothing recognizable: a plain Node image.
    Shell,
}

/// Does the build command run `hugo`?
fn runs_hugo(build: &str) -> bool {
    build.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))).any(|t| {
        let t = t.rsplit('/').next().unwrap_or(t);
        t == "hugo"
    })
}

fn static_build_kind(dir: &Path, runtime: Runtime, build: &str) -> StaticBuild {
    let has = |f: &str| dir.join(f).is_file();
    let hugo = runs_hugo(build) || ["hugo.toml", "hugo.yaml", "hugo.yml", "hugo.json"].iter().any(|f| has(f));
    // An explicit (or detected) language runtime picks the toolchain.
    match runtime {
        Runtime::Node if has("package.json") => return StaticBuild::Node,
        Runtime::Node => return StaticBuild::Shell,
        Runtime::Python => return StaticBuild::Python,
        Runtime::Ruby => return StaticBuild::Ruby,
        Runtime::Go if hugo => return StaticBuild::Hugo,
        Runtime::Go => return StaticBuild::Go,
        _ => {}
    }
    if has("package.json") {
        StaticBuild::Node
    } else if has("requirements.txt") || has("pyproject.toml") || has("Pipfile") {
        StaticBuild::Python
    } else if has("Gemfile") {
        StaticBuild::Ruby
    } else if hugo {
        StaticBuild::Hugo
    } else if has("go.mod") {
        StaticBuild::Go
    } else {
        StaticBuild::Shell
    }
}

/// The `build` stage of a static site: toolchain, dependencies, then the
/// build command.
fn static_build_stage(df: &mut Df, dir: &Path, kind: StaticBuild, plan: Option<NodePlan>, build: &str) {
    let hugo_apk = runs_hugo(build).then_some(HUGO_APK);
    match kind {
        StaticBuild::Node | StaticBuild::Shell => {
            let plan = plan.unwrap_or_else(NodePlan::bare);
            node_build_stage(df, &plan, Some("build"), Some(build), hugo_apk);
        }
        StaticBuild::Python => {
            python_stage(df, dir, Some("build"));
            df.run_env(&[], build);
        }
        StaticBuild::Ruby => {
            ruby_stage(df, dir, Some("build"));
            df.run_env(&[], build);
        }
        StaticBuild::Hugo => {
            df.line("FROM alpine:3.20 AS build");
            // Hugo modules need the Go toolchain.
            let apk = if exists(dir, "go.mod") { format!("{HUGO_APK} go") } else { HUGO_APK.to_string() };
            df.run(&[], &apk);
            df.line("WORKDIR /src");
            df.declare_build_env();
            df.blank();
            df.line("COPY . .");
            df.run_env(&[], build);
        }
        StaticBuild::Go => {
            go_source_stage(df, dir);
            df.run_steps_env(&go_caches(), &[(true, build.to_string())]);
        }
    }
}

fn static_site(dir: &Path, runtime: Runtime, opts: &DockerfileOptions) -> Result<GeneratedDockerfile> {
    let publish = publish_dir(opts)?;
    let has_pkg = dir.join("package.json").is_file();
    let plan = if has_pkg { Some(node_plan(dir)?) } else { None };
    let build = nonempty(&opts.build_command)
        .map(str::to_string)
        .or_else(|| plan.as_ref().filter(|p| p.has_build_script).map(|p| p.pm.run_script("build")));

    let mut df = Df::new("static site served by nginx", opts);
    match build {
        Some(build) => {
            let kind = static_build_kind(dir, runtime, &build);
            let plan = if kind == StaticBuild::Node { plan } else { None };
            static_build_stage(&mut df, dir, kind, plan, &build);
            // The output directory only exists after the build: pick it then.
            let select = match &publish {
                Publish::Dir(p) => format!(
                    "mkdir -p /ferry-publish && if [ -d \"{p}\" ]; then cp -a \"{p}/.\" /ferry-publish/; \
                     else echo \"error: publish directory '{p}' not found after the build\" >&2; exit 1; fi"
                ),
                // An explicit `.` wins over any build output directory.
                Publish::Root => format!(
                    "mkdir -p /ferry-publish && echo \"Publishing the root directory\" && cp -a ./. /ferry-publish/ \
                     && {}",
                    strip_ferry_files("/ferry-publish")
                ),
                Publish::Auto => {
                    let list = PUBLISH_CANDIDATES.join(" ");
                    format!(
                        "mkdir -p /ferry-publish && for d in {list}; do if [ -d \"$d\" ]; then \
                         echo \"Publishing ./$d\"; cp -a \"$d/.\" /ferry-publish/; exit 0; fi; done; \
                         echo \"error: no build output found (looked for {list}); set the publish directory\" >&2; exit 1"
                    )
                }
            };
            df.run(&[], &select);
            df.blank();
            nginx_stage(&mut df);
            df.line(format!("COPY --from=build /ferry-publish/ {NGINX_HTML}/"));
        }
        None => {
            nginx_stage(&mut df);
            // No build step: an explicit publish dir (`.` included), else the
            // context root when it holds index.html, else a committed
            // dist/build/out/public.
            let publish = match publish {
                Publish::Auto if dir.join("index.html").is_file() => Publish::Root,
                Publish::Auto => PUBLISH_CANDIDATES
                    .iter()
                    .find(|c| dir.join(c).join("index.html").is_file())
                    .map_or(Publish::Root, |c| Publish::Dir(c.to_string())),
                explicit => explicit,
            };
            match &publish {
                Publish::Dir(p) => {
                    if !dir.join(p).is_dir() {
                        return Err(Error::invalid(format!("publish directory '{p}' does not exist")));
                    }
                    df.line(format!("COPY {p}/ {NGINX_HTML}/"));
                }
                Publish::Root | Publish::Auto => {
                    df.line(format!("COPY . {NGINX_HTML}/"));
                    df.run(&[], &strip_ferry_files(NGINX_HTML));
                }
            }
        }
    }
    df.line("EXPOSE 80");
    // nginx: nothing of the project's is started.
    Ok(GeneratedDockerfile { contents: df.finish(), port_hint: Some(80), start: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let p = d.path().join(name);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, contents).unwrap();
        }
        d
    }

    fn gen_with(runtime: Runtime, files: &[(&str, &str)], opts: DockerfileOptions) -> Result<GeneratedDockerfile> {
        let d = project(files);
        generate(runtime, d.path(), &opts)
    }

    fn gen_ok(runtime: Runtime, files: &[(&str, &str)], opts: DockerfileOptions) -> String {
        match gen_with(runtime, files, opts) {
            Ok(g) => g.contents,
            Err(e) => panic!("generation failed: {e}"),
        }
    }

    fn has_line(df: &str, line: &str) -> bool {
        df.lines().any(|l| l.trim() == line)
    }

    fn pos(df: &str, needle: &str) -> usize {
        df.find(needle).unwrap_or_else(|| panic!("'{needle}' not found in:\n{df}"))
    }

    fn opts() -> DockerfileOptions {
        DockerfileOptions::default()
    }

    /// The whole instruction (continuation lines joined) containing `needle`.
    fn instruction(df: &str, needle: &str) -> String {
        let mut current = String::new();
        for line in df.lines() {
            current.push_str(line.trim());
            current.push(' ');
            if !line.trim_end().ends_with('\\') {
                if current.contains(needle) {
                    return current.trim().to_string();
                }
                current.clear();
            }
        }
        panic!("no instruction containing '{needle}' in:\n{df}")
    }

    fn secret(key: &str) -> String {
        format!("--mount=type=secret,id={key},env={key}")
    }

    #[test]
    fn node_npm_ci_with_build_and_start_script() {
        let df = gen_ok(
            Runtime::Node,
            &[
                (
                    "package.json",
                    r#"{"engines":{"node":"20.x"},"scripts":{"build":"tsc","start":"node dist/index.js"}}"#,
                ),
                ("package-lock.json", "{}"),
            ],
            DockerfileOptions { build_arg_keys: vec!["API_KEY".into(), "PATH".into(), "BAD-KEY".into()], ..opts() },
        );
        assert!(has_line(&df, "FROM node:20-alpine"), "{df}");
        assert!(has_line(&df, "COPY package.json package-lock.json ./"), "{df}");
        assert!(has_line(&df, r#"CMD ["npm","start"]"#), "{df}");
        // Env vars reach install and build through secret mounts, never ARG
        // (ARG values end up in the image history).
        let install = instruction(&df, "npm ci --include=dev --no-audit --no-fund");
        let build = instruction(&df, "npm run build");
        for step in [&install, &build] {
            assert!(step.starts_with("RUN ") && step.contains(&secret("API_KEY")), "{step}");
            assert!(!step.contains("PATH") && !step.contains("BAD-KEY"), "{step}");
        }
        assert!(!df.contains("ARG API_KEY") && !df.contains("ARG PATH"), "{df}");
        assert!(has_line(&df, "ARG FERRY_BUILD_ENV_DIGEST"), "{df}");
        // manifests → install → source → NODE_ENV → build
        assert!(pos(&df, "COPY package.json") < pos(&df, "npm ci"));
        assert!(pos(&df, "npm ci") < pos(&df, "COPY . ."));
        assert!(pos(&df, "npm ci") < pos(&df, "ENV NODE_ENV=production"));
        assert!(pos(&df, "ENV NODE_ENV=production") < pos(&df, "npm run build"));
        assert!(pos(&df, "ARG FERRY_BUILD_ENV_DIGEST") < pos(&df, "COPY package.json"));

        // Without build-time env: plain RUN lines and no digest.
        let df = gen_ok(
            Runtime::Node,
            &[("package.json", r#"{"scripts":{"build":"tsc","start":"node x"}}"#), ("package-lock.json", "{}")],
            opts(),
        );
        assert!(has_line(&df, "RUN npm ci --include=dev --no-audit --no-fund"), "{df}");
        assert!(has_line(&df, "RUN npm run build"), "{df}");
        assert!(!df.contains("ARG") && !df.contains("type=secret"), "{df}");
    }

    #[test]
    fn node_package_managers() {
        let yarn = gen_ok(
            Runtime::Node,
            &[("package.json", r#"{"main":"server.js"}"#), ("yarn.lock", ""), ("server.js", "")],
            opts(),
        );
        assert!(yarn.contains("corepack enable"), "{yarn}");
        assert!(has_line(&yarn, "RUN yarn install --frozen-lockfile --production=false"), "{yarn}");
        assert!(has_line(&yarn, r#"CMD ["node","server.js"]"#), "{yarn}");

        let berry = gen_ok(
            Runtime::Node,
            &[
                ("package.json", r#"{"packageManager":"yarn@4.1.0","scripts":{"start":"node x.js"}}"#),
                ("yarn.lock", "__metadata:\n  version: 8\n"),
                (".yarnrc.yml", "nodeLinker: node-modules\n"),
                (".yarn/releases/yarn.cjs", ""),
            ],
            opts(),
        );
        assert!(has_line(&berry, "RUN yarn install --immutable"), "{berry}");
        assert!(has_line(&berry, "COPY .yarn ./.yarn"), "{berry}");
        assert!(has_line(&berry, r#"CMD ["yarn","start"]"#), "{berry}");

        let pnpm = gen_ok(
            Runtime::Node,
            &[("package.json", r#"{"scripts":{"build":"vite build","start":"node s.js"}}"#), ("pnpm-lock.yaml", "")],
            opts(),
        );
        assert!(pnpm.contains("corepack enable"), "{pnpm}");
        assert!(has_line(&pnpm, "RUN pnpm install --frozen-lockfile"), "{pnpm}");
        assert!(has_line(&pnpm, "RUN pnpm run build"), "{pnpm}");

        let bun = gen_ok(
            Runtime::Node,
            &[("package.json", r#"{"scripts":{"start":"bun x.ts"}}"#), ("bun.lockb", "")],
            opts(),
        );
        assert!(has_line(&bun, "FROM oven/bun:1-alpine"), "{bun}");
        assert!(has_line(&bun, "RUN bun install"), "{bun}");
        assert!(has_line(&bun, r#"CMD ["bun","run","start"]"#), "{bun}");

        let npm = gen_ok(Runtime::Node, &[("package.json", "{}"), ("index.js", "")], opts());
        assert!(has_line(&npm, "RUN npm install --include=dev --no-audit --no-fund"), "{npm}");
        assert!(has_line(&npm, "FROM node:22-alpine"), "{npm}");
        assert!(has_line(&npm, r#"CMD ["node","index.js"]"#), "{npm}");
        assert!(!npm.contains("run build"), "{npm}");
    }

    #[test]
    fn node_start_resolution() {
        // start_command wins, in shell form (so $PORT expands)
        let df = gen_ok(
            Runtime::Node,
            &[("package.json", r#"{"scripts":{"start":"node a.js"}}"#), ("Procfile", "web: node web.js")],
            DockerfileOptions { start_command: Some("node x.js --port $PORT".into()), ..opts() },
        );
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","node x.js --port $PORT"]"#), "{df}");
        // Procfile before the start script
        let df = gen_ok(
            Runtime::Node,
            &[
                ("package.json", r#"{"scripts":{"start":"node a.js"}}"#),
                ("Procfile", "web: node web.js\nworker: node w.js"),
            ],
            opts(),
        );
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","node web.js"]"#), "{df}");
        let df = gen_ok(
            Runtime::Node,
            &[("package.json", "{}"), ("Procfile", "web: node web.js\nworker: node w.js")],
            DockerfileOptions { service_type: Some(ServiceType::BackgroundWorker), ..opts() },
        );
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","node w.js"]"#), "{df}");
        // main field
        let df = gen_ok(Runtime::Node, &[("package.json", r#"{"main":"lib/main.js"}"#), ("lib/main.js", "")], opts());
        assert!(has_line(&df, r#"CMD ["node","lib/main.js"]"#), "{df}");
        // nothing → helpful error
        let err = gen_with(Runtime::Node, &[("package.json", "{}")], opts()).unwrap_err().to_string();
        assert!(err.contains("start command") && err.contains("Procfile"), "{err}");
        // cron jobs may omit it
        let df = gen_ok(
            Runtime::Node,
            &[("package.json", "{}")],
            DockerfileOptions { service_type: Some(ServiceType::CronJob), ..opts() },
        );
        assert!(!df.contains("CMD"), "{df}");
    }

    /// The build log says which command the image starts and where it comes
    /// from: a default nobody chose is what to look at when an instance
    /// doesn't stay up.
    #[test]
    fn the_start_command_and_its_origin_are_reported() {
        let start = |runtime: Runtime, files: &[(&str, &str)], opts: DockerfileOptions| -> Option<String> {
            gen_with(runtime, files, opts).unwrap().start.map(|s| s.log_line())
        };
        let line = |runtime: Runtime, files: &[(&str, &str)]| start(runtime, files, opts()).unwrap();

        // What the user chose: the service's start command, then the Procfile.
        let chosen = start(
            Runtime::Node,
            &[("package.json", r#"{"scripts":{"start":"node a.js"}}"#), ("Procfile", "web: node web.js")],
            DockerfileOptions { start_command: Some("  node x.js --port $PORT ".into()), ..opts() },
        );
        assert_eq!(chosen.as_deref(), Some("==> Start command: node x.js --port $PORT (the service's start command)"));
        assert_eq!(
            line(Runtime::Node, &[("package.json", "{}"), ("Procfile", "web: node web.js")]),
            "==> Start command: node web.js (the Procfile)"
        );

        // What Ferry falls back to, and why.
        assert_eq!(
            line(Runtime::Node, &[("package.json", r#"{"scripts":{"start":"node a.js"}}"#)]),
            "==> Start command: npm start (the \"start\" script of package.json)"
        );
        // A library: a build, a "main" the build produces, nothing to start.
        assert_eq!(
            line(Runtime::Node, &[("package.json", r#"{"main":"./dist/index.js","scripts":{"build":"vite build"}}"#)]),
            "==> Start command: node ./dist/index.js (package.json has no \"start\" script: its \"main\" file is run)"
        );
        assert_eq!(
            line(Runtime::Node, &[("package.json", "{}"), ("server.js", "")]),
            "==> Start command: node server.js (package.json has no \"start\" script: server.js is run)"
        );
        assert_eq!(
            line(Runtime::Python, &[("requirements.txt", ""), ("app.py", "")]),
            "==> Start command: python app.py (the Python default: app.py is run)"
        );
        assert_eq!(
            line(Runtime::Go, &[("go.mod", "module example.com/app\n\ngo 1.23\n"), ("main.go", "package main\n")]),
            "==> Start command: /usr/local/bin/app (the Go default: the built binary is run)"
        );
        assert_eq!(
            line(Runtime::Rust, &[("Cargo.toml", "[package]\nname = \"api\"\n"), ("src/main.rs", "fn main() {}")]),
            "==> Start command: /usr/local/bin/api (the Rust default: the package's binary is run)"
        );
        assert_eq!(
            line(Runtime::Ruby, &[("Gemfile", ""), ("config.ru", "")]),
            "==> Start command: bundle exec rackup -o 0.0.0.0 -p ${PORT:-10000} (the Ruby default for a Rack app: \
             config.ru is served)"
        );

        // Nothing of the project's is started: nginx, or a cron job whose
        // command comes at run time.
        assert_eq!(start(Runtime::Static, &[("index.html", "")], opts()), None);
        let cron = DockerfileOptions { service_type: Some(ServiceType::CronJob), ..opts() };
        assert_eq!(start(Runtime::Node, &[("package.json", "{}")], cron), None);
    }

    #[test]
    fn node_copies_everything_when_install_needs_it() {
        let df = gen_ok(
            Runtime::Node,
            &[
                ("package.json", r#"{"scripts":{"postinstall":"prisma generate","start":"node a"}}"#),
                ("package-lock.json", ""),
            ],
            opts(),
        );
        assert!(pos(&df, "COPY . .") < pos(&df, "RUN npm ci"), "{df}");
        let df = gen_ok(
            Runtime::Node,
            &[("package.json", r#"{"workspaces":["packages/*"],"scripts":{"start":"node a"}}"#)],
            opts(),
        );
        assert!(pos(&df, "COPY . .") < pos(&df, "RUN npm install"), "{df}");
        assert!(gen_with(Runtime::Node, &[("package.json", "{not json")], opts()).is_err());
    }

    #[test]
    fn multiline_commands_use_exec_form() {
        let df = gen_ok(
            Runtime::Node,
            &[("package.json", "{}")],
            DockerfileOptions {
                build_command: Some("echo one\necho two".into()),
                start_command: Some("node a.js\nFROM evil".into()),
                ..opts()
            },
        );
        assert!(has_line(&df, r#"RUN ["/bin/sh","-c","echo one\necho two"]"#), "{df}");
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","node a.js\nFROM evil"]"#), "{df}");
        assert!(!df.lines().any(|l| l.starts_with("FROM evil")), "{df}");
    }

    #[test]
    fn python_variants() {
        let df = gen_ok(
            Runtime::Python,
            &[("requirements.txt", "flask\n"), ("app.py", ""), (".python-version", "3.11.6\n")],
            DockerfileOptions { build_arg_keys: vec!["SECRET".into()], ..opts() },
        );
        assert!(has_line(&df, "FROM python:3.11-slim"), "{df}");
        assert!(df.contains("PYTHONUNBUFFERED=1"), "{df}");
        assert!(has_line(&df, "COPY requirements.txt ./"), "{df}");
        let install = instruction(&df, "pip install --no-cache-dir -r requirements.txt");
        assert!(install.contains(&secret("SECRET")), "{install}");
        assert!(has_line(&df, r#"CMD ["python","app.py"]"#), "{df}");
        assert!(!df.contains("ARG SECRET"), "{df}");
        assert!(pos(&df, "ARG FERRY_BUILD_ENV_DIGEST") < pos(&df, "pip install"), "{df}");
        assert!(!df.contains("apt-get"), "no system packages without database drivers: {df}");

        // A pyproject.toml holding tool settings only: nothing to install.
        let df = gen_ok(
            Runtime::Python,
            &[
                ("pyproject.toml", "[tool.black]\nline-length = 100\n"),
                ("main.py", ""),
                ("runtime.txt", "python-3.10.2"),
            ],
            opts(),
        );
        assert!(has_line(&df, "FROM python:3.10-slim"), "{df}");
        assert!(!df.contains("pip install"), "{df}");
        assert!(has_line(&df, "COPY . ."), "{df}");
        assert!(has_line(&df, r#"CMD ["python","main.py"]"#), "{df}");

        let df = gen_ok(
            Runtime::Python,
            &[("Pipfile", ""), ("Pipfile.lock", "{}"), ("Procfile", "web: gunicorn app:app -b 0.0.0.0:$PORT")],
            opts(),
        );
        assert!(has_line(&df, "FROM python:3.12-slim"), "{df}");
        assert!(has_line(&df, "RUN pip install --no-cache-dir pipenv"), "{df}");
        assert!(has_line(&df, "RUN pipenv install --system --deploy"), "{df}");
        assert!(pos(&df, "pip install --no-cache-dir pipenv") < pos(&df, "COPY Pipfile Pipfile.lock ./"), "{df}");
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","gunicorn app:app -b 0.0.0.0:$PORT"]"#), "{df}");

        let df =
            gen_ok(Runtime::Python, &[("requirements.txt", "-r base.txt\n"), ("base.txt", ""), ("app.py", "")], opts());
        assert!(pos(&df, "COPY . .") < pos(&df, "RUN pip install"), "{df}");

        let err = gen_with(Runtime::Python, &[("requirements.txt", "")], opts()).unwrap_err().to_string();
        assert!(err.contains("start command"), "{err}");
    }

    #[test]
    fn go_variants() {
        let df = gen_ok(
            Runtime::Go,
            &[("go.mod", "module x\n\ngo 1.22.1\n"), ("go.sum", ""), ("main.go", "package main")],
            DockerfileOptions { build_arg_keys: vec!["GOPRIVATE".into()], ..opts() },
        );
        assert!(has_line(&df, "FROM golang:1.22-alpine AS build"), "{df}");
        assert!(has_line(&df, "COPY go.mod go.sum ./"), "{df}");
        assert!(df.contains("go mod download"), "{df}");
        assert!(df.contains("CGO_ENABLED=0 go build") && df.contains("-o /out/app ."), "{df}");
        assert!(has_line(&df, "FROM alpine:3.20"), "{df}");
        assert!(df.contains("ca-certificates"), "{df}");
        assert!(has_line(&df, r#"CMD ["/usr/local/bin/app"]"#), "{df}");
        assert!(!df.contains("ARG GOPRIVATE"), "{df}");
        assert!(instruction(&df, "go mod download").contains(&secret("GOPRIVATE")), "{df}");
        assert!(instruction(&df, "CGO_ENABLED=0 go build").contains(&secret("GOPRIVATE")), "{df}");
        assert!(!instruction(&df, "apk add").contains("type=secret"), "{df}");

        let df = gen_ok(
            Runtime::Go,
            &[("go.mod", "module x\n"), ("cmd/server/main.go", "package main")],
            DockerfileOptions {
                build_command: Some("go build -o /out/srv ./cmd/server".into()),
                start_command: Some("srv --port $PORT".into()),
                ..opts()
            },
        );
        assert!(has_line(&df, "FROM golang:1.23-alpine AS build"), "{df}");
        assert!(df.contains("( go build -o /out/srv ./cmd/server )"), "{df}");
        assert!(!df.contains("CGO_ENABLED=0 go build"), "{df}");
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","srv --port $PORT"]"#), "{df}");

        let df = gen_ok(Runtime::Go, &[("go.mod", "module x\n"), ("cmd/api/main.go", "package main")], opts());
        assert!(df.contains("-o /out/app ./cmd/api"), "{df}");
        assert!(df.contains("RUN --mount=type=cache,target=/go/pkg/mod \\\n    --mount=type=cache,target=/root/.cache/go-build \\\n    mkdir -p /out && CGO_ENABLED=0"), "{df}");

        // A comment in a chained user command must not swallow the rest of the chain.
        let df = gen_ok(
            Runtime::Go,
            &[("go.mod", "module x\n"), ("main.go", "package main")],
            DockerfileOptions { build_command: Some("go build -o /out/app . # static".into()), ..opts() },
        );
        assert!(df.contains(r#"["/bin/sh","-c","set -e\nmkdir -p /out\ngo build -o /out/app . # static\n"]"#), "{df}");
    }

    #[test]
    fn rust_variants() {
        let df = gen_ok(
            Runtime::Rust,
            &[
                ("Cargo.toml", "[package]\nname = \"web-api\"\nversion = \"0.1.0\"\n"),
                ("Cargo.lock", "[[package]]\nname = \"openssl-sys\"\n"),
                ("src/main.rs", "fn main() {}"),
            ],
            opts(),
        );
        assert!(has_line(&df, "FROM rust:1-slim-bookworm AS build"), "{df}");
        assert!(df.contains("cargo build --release"), "{df}");
        assert!(df.contains("cp target/release/web-api /out/"), "{df}");
        assert!(has_line(&df, "FROM debian:bookworm-slim"), "{df}");
        assert!(df.contains("libssl-dev") && df.contains("libssl3"), "{df}");
        assert!(has_line(&df, r#"CMD ["/usr/local/bin/web-api"]"#), "{df}");

        let df = gen_ok(
            Runtime::Rust,
            &[("Cargo.toml", "[package]\nname = \"x\"\n\n[[bin]]\nname = \"srv\"\npath = \"src/srv.rs\"\n")],
            opts(),
        );
        assert!(df.contains("cp target/release/srv /out/"), "{df}");
        assert!(!df.contains("libssl"), "{df}");

        let err = gen_with(Runtime::Rust, &[("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n")], opts()).unwrap_err();
        assert!(err.to_string().contains("start command"), "{err}");
        let df = gen_ok(
            Runtime::Rust,
            &[("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n")],
            DockerfileOptions { start_command: Some("api".into()), ..opts() },
        );
        assert!(df.contains("find target/release -maxdepth 1"), "{df}");
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","api"]"#), "{df}");
    }

    #[test]
    fn ruby_variants() {
        let df = gen_ok(
            Runtime::Ruby,
            &[("Gemfile", "source 'https://rubygems.org'\ngem 'rack'\n"), ("Gemfile.lock", ""), ("config.ru", "")],
            opts(),
        );
        assert!(has_line(&df, "FROM ruby:3.3-slim"), "{df}");
        assert!(df.contains("build-essential"), "{df}");
        assert!(has_line(&df, "COPY Gemfile Gemfile.lock ./"), "{df}");
        assert!(has_line(&df, "RUN bundle install"), "{df}");
        assert!(df.contains(r#"CMD ["/bin/sh","-c","bundle exec rackup -o 0.0.0.0 -p ${PORT:-10000}"]"#), "{df}");

        let df = gen_ok(
            Runtime::Ruby,
            &[("Gemfile", "ruby '3.2.2'\n"), ("Procfile", "web: bundle exec puma -p $PORT")],
            opts(),
        );
        assert!(has_line(&df, "FROM ruby:3.2-slim"), "{df}");
        assert!(has_line(&df, r#"CMD ["/bin/sh","-c","bundle exec puma -p $PORT"]"#), "{df}");

        let err = gen_with(Runtime::Ruby, &[("Gemfile", "")], opts()).unwrap_err().to_string();
        assert!(err.contains("config.ru"), "{err}");
    }

    #[test]
    fn static_plain_and_publish_dir() {
        let g = gen_with(Runtime::Static, &[("index.html", "<h1>hi</h1>")], opts()).unwrap();
        assert_eq!(g.port_hint, Some(80));
        let df = g.contents;
        assert!(has_line(&df, "FROM nginx:alpine"), "{df}");
        assert!(has_line(&df, "COPY . /usr/share/nginx/html/"), "{df}");
        assert!(df.contains("try_files $uri $uri/ $uri.html =404;"), "{df}");
        assert!(df.contains("listen 80;") && df.contains("gzip on;"), "{df}");
        assert!(df.contains("root /usr/share/nginx/html;"), "{df}");
        assert!(!df.contains("node:"), "{df}");
        // Dotfiles are never served, /.well-known/ is.
        assert!(df.contains("'    location ~ /\\.(?!well-known(/|$)) {' \\\n    '        return 404;'"), "{df}");

        let df = gen_ok(
            Runtime::Static,
            &[("site/index.html", "")],
            DockerfileOptions { publish_dir: Some("./site/".into()), ..opts() },
        );
        assert!(has_line(&df, "COPY site/ /usr/share/nginx/html/"), "{df}");

        let err = gen_with(Runtime::Static, &[], DockerfileOptions { publish_dir: Some("nope".into()), ..opts() });
        assert!(err.unwrap_err().to_string().contains("does not exist"));
        let err = gen_with(Runtime::Static, &[], DockerfileOptions { publish_dir: Some("../x".into()), ..opts() });
        assert!(err.is_err());
        let err = gen_with(Runtime::Static, &[], DockerfileOptions { publish_dir: Some("a;rm -rf".into()), ..opts() });
        assert!(err.is_err());
    }

    #[test]
    fn static_with_node_build() {
        let g = gen_with(
            Runtime::Node,
            &[("package.json", r#"{"scripts":{"build":"vite build"}}"#), ("package-lock.json", "")],
            DockerfileOptions {
                service_type: Some(ServiceType::StaticSite),
                build_arg_keys: vec!["VITE_API".into()],
                ..opts()
            },
        )
        .unwrap();
        assert_eq!(g.port_hint, Some(80));
        let df = g.contents;
        assert!(has_line(&df, "FROM node:22-alpine AS build"), "{df}");
        assert!(instruction(&df, "npm ci --include=dev --no-audit --no-fund").contains(&secret("VITE_API")), "{df}");
        assert!(instruction(&df, "npm run build").contains(&secret("VITE_API")), "{df}");
        // The publish step runs no project code: no env.
        assert!(!instruction(&df, "/ferry-publish &&").contains("type=secret"), "{df}");
        assert!(df.contains("for d in dist build out public _site site"), "{df}");
        assert!(has_line(&df, "FROM nginx:alpine"), "{df}");
        assert!(has_line(&df, "COPY --from=build /ferry-publish/ /usr/share/nginx/html/"), "{df}");
        assert!(!df.contains("ARG VITE_API"), "{df}");
        assert!(pos(&df, "ARG FERRY_BUILD_ENV_DIGEST") < pos(&df, "npm run build"), "{df}");
        assert!(pos(&df, "FROM node:22-alpine AS build") < pos(&df, "FROM nginx:alpine"), "{df}");

        let df = gen_ok(
            Runtime::Static,
            &[("package.json", r#"{"scripts":{"build":"x"}}"#)],
            DockerfileOptions { publish_dir: Some("public/out".into()), ..opts() },
        );
        assert!(df.contains("if [ -d \"public/out\" ]"), "{df}");

        // package.json without a build script: served as-is
        let df = gen_ok(Runtime::Static, &[("package.json", "{}"), ("index.html", "")], opts());
        assert!(!df.contains("AS build"), "{df}");
        assert!(has_line(&df, "COPY . /usr/share/nginx/html/"), "{df}");

        // No build and no root index.html: a committed output dir is served.
        let df = gen_ok(Runtime::Static, &[("public/index.html", ""), ("README.md", "")], opts());
        assert!(has_line(&df, "COPY public/ /usr/share/nginx/html/"), "{df}");
        // ...but a root index.html wins over asset folders.
        let df = gen_ok(Runtime::Static, &[("index.html", ""), ("public/index.html", "")], opts());
        assert!(has_line(&df, "COPY . /usr/share/nginx/html/"), "{df}");

        // explicit build command without package.json
        let df = gen_ok(
            Runtime::Static,
            &[("build.sh", "")],
            DockerfileOptions { build_command: Some("sh build.sh".into()), publish_dir: Some("dist".into()), ..opts() },
        );
        assert!(df.contains("AS build") && has_line(&df, "RUN sh build.sh"), "{df}");
    }

    #[test]
    fn explicit_root_publish_dir_is_honored() {
        let root = || DockerfileOptions { publish_dir: Some(".".into()), ..opts() };
        // Regression: a vite-like app (build script → dist/, root
        // index.html) with publish_dir '.' served dist/ anyway.
        let vite = [
            ("package.json", r#"{"scripts":{"build":"vite build"}}"#),
            ("package-lock.json", ""),
            ("index.html", "<script src=\"/dist/app.js\"></script>"),
            ("dist/index.html", "built"),
        ];
        for publish in [".", "./", " ./ "] {
            let df = gen_ok(Runtime::Static, &vite, DockerfileOptions { publish_dir: Some(publish.into()), ..opts() });
            assert!(has_line(&df, "RUN npm run build"), "{df}");
            let step = instruction(&df, "/ferry-publish &&");
            assert!(step.contains("cp -a ./. /ferry-publish/"), "{step}");
            assert!(step.contains("rm -rf /ferry-publish/Dockerfile.ferry* /ferry-publish/.git"), "{step}");
            assert!(!df.contains("for d in dist"), "{df}");
            assert!(has_line(&df, "COPY --from=build /ferry-publish/ /usr/share/nginx/html/"), "{df}");
        }
        // Without a build step, `.` wins over committed output directories.
        for files in
            [&[("public/index.html", ""), ("README.md", "")][..], &[("dist/index.html", ""), ("index.html", "")][..]]
        {
            let df = gen_ok(Runtime::Static, files, root());
            assert!(has_line(&df, "COPY . /usr/share/nginx/html/"), "{df}");
            assert!(!df.contains("COPY public/") && !df.contains("COPY dist/"), "{df}");
            assert!(df.contains("rm -rf /usr/share/nginx/html/Dockerfile.ferry* /usr/share/nginx/html/.git"), "{df}");
        }
        // Unset still auto-detects (unchanged behavior).
        let df = gen_ok(Runtime::Static, &[("public/index.html", ""), ("README.md", "")], opts());
        assert!(has_line(&df, "COPY public/ /usr/share/nginx/html/"), "{df}");
        let df = gen_ok(Runtime::Static, &vite, opts());
        assert!(df.contains("for d in dist build out public _site site"), "{df}");
        // An explicit directory is honored even if another candidate exists.
        let df = gen_ok(
            Runtime::Static,
            &[("public/index.html", ""), ("site/index.html", "")],
            DockerfileOptions { publish_dir: Some("site".into()), ..opts() },
        );
        assert!(has_line(&df, "COPY site/ /usr/share/nginx/html/"), "{df}");
    }

    #[test]
    fn unsupported_runtimes() {
        let d = tempfile::tempdir().unwrap();
        assert!(generate(Runtime::Docker, d.path(), &opts()).is_err());
        assert!(generate(Runtime::Image, d.path(), &opts()).is_err());
        assert!(generate(Runtime::Auto, d.path(), &opts()).is_err());
        let d = project(&[("index.html", "")]);
        assert_eq!(generate(Runtime::Auto, d.path(), &opts()).unwrap().port_hint, Some(80));
    }

    #[test]
    fn build_arg_names() {
        assert!(usable_build_arg("NODE_ENV"));
        assert!(usable_build_arg("_X1"));
        assert!(!usable_build_arg("1X"));
        assert!(!usable_build_arg("A-B"));
        assert!(!usable_build_arg("A B"));
        assert!(!usable_build_arg(""));
        assert!(!usable_build_arg("PATH"));
        assert!(!usable_build_arg("DOCKER_HOST"));
        assert!(!usable_build_arg("BUILDKIT_INLINE_CACHE"));
        assert!(!usable_build_arg("FERRY_BUILD_ENV_DIGEST"));
        assert!(usable_build_arg("FERRY_SERVICE_NAME"));
        assert_eq!(build_env_keys(&["A".into(), "PATH".into(), "A".into(), "b_2".into()]), vec!["A", "b_2"]);
        assert_eq!(secret_id("API_KEY"), "API_KEY");
        assert_eq!(secret_id("a,b=c d"), "a_b_c_d");
    }

    /// Env values must never be declared as ARG in any generated Dockerfile
    /// (BuildKit writes ARG values into the image history).
    #[test]
    fn build_env_is_never_an_arg() {
        let keys = DockerfileOptions { build_arg_keys: vec!["API_SECRET".into()], ..opts() };
        // (runtime, files, service type, step that must see the env)
        type Case<'a> = (Runtime, Vec<(&'a str, &'a str)>, Option<ServiceType>, &'a str);
        let cases: Vec<Case> = vec![
            (Runtime::Node, vec![("package.json", r#"{"scripts":{"start":"node a"}}"#)], None, "npm install"),
            (Runtime::Python, vec![("requirements.txt", "flask"), ("app.py", "")], None, "pip install"),
            (Runtime::Go, vec![("go.mod", "module x\n"), ("main.go", "package main")], None, "go build"),
            (
                Runtime::Rust,
                vec![("Cargo.toml", "[package]\nname = \"x\"\n"), ("src/main.rs", "")],
                None,
                "cargo build",
            ),
            (Runtime::Ruby, vec![("Gemfile", ""), ("config.ru", "")], None, "bundle install"),
            (Runtime::Static, vec![("requirements.txt", "mkdocs")], Some(ServiceType::StaticSite), "mkdocs build"),
        ];
        for (runtime, files, service_type, step) in cases {
            let opts = DockerfileOptions {
                service_type,
                build_command: (runtime == Runtime::Static).then(|| "mkdocs build".to_string()),
                ..keys.clone()
            };
            let df = gen_ok(runtime, &files, opts);
            assert!(!df.lines().any(|l| l.trim_start().starts_with("ARG API_SECRET")), "{df}");
            assert!(!df.contains("ENV API_SECRET"), "{df}");
            assert!(instruction(&df, step).contains(&secret("API_SECRET")), "{runtime}: {df}");
            assert!(pos(&df, "ARG FERRY_BUILD_ENV_DIGEST") < pos(&df, step), "{df}");
        }
        // Static sites without a build step run no project code at all.
        let df = gen_ok(Runtime::Static, &[("index.html", "")], keys);
        assert!(!df.contains("API_SECRET") && !df.contains("ARG"), "{df}");
    }

    #[test]
    fn explicit_runtime_must_match_the_source() {
        let err = gen_with(Runtime::Node, &[("requirements.txt", ""), ("app.py", "")], opts()).unwrap_err().to_string();
        assert_eq!(
            err,
            "runtime 'node' was selected but the root directory has no package.json \
             (it looks like a Python project: set the runtime to 'python' or 'auto')"
        );
        let err = gen_with(Runtime::Ruby, &[("package.json", "{}")], opts()).unwrap_err().to_string();
        assert!(err.contains("has no Gemfile") && err.contains("Node.js project"), "{err}");
        let err = gen_with(Runtime::Go, &[("Dockerfile", "FROM x")], opts()).unwrap_err().to_string();
        assert!(err.contains("has no go.mod") && err.contains("runtime to 'docker'"), "{err}");
        let err = gen_with(Runtime::Rust, &[], opts()).unwrap_err().to_string();
        assert_eq!(err, "runtime 'rust' was selected but the root directory has no Cargo.toml");
        let err = gen_with(Runtime::Python, &[("README.md", "")], opts()).unwrap_err().to_string();
        assert!(err.contains("no requirements.txt, pyproject.toml, Pipfile or .py file"), "{err}");
        // A lone script is a valid Python app.
        assert!(gen_with(Runtime::Python, &[("main.py", "")], opts()).is_ok());
    }

    #[test]
    fn build_command_does_not_skip_the_runtime_check() {
        // Regression: `--runtime go|rust --build-cmd X` on a Python project
        // "built" fine, then crashed at start (or failed on target/release).
        let custom = |b: &str| DockerfileOptions {
            build_command: Some(b.into()),
            start_command: Some("./app".into()),
            ..opts()
        };
        let python = [("requirements.txt", "flask\n"), ("app.py", "")];
        for (runtime, manifest) in [
            (Runtime::Go, "go.mod"),
            (Runtime::Rust, "Cargo.toml"),
            (Runtime::Node, "package.json"),
            (Runtime::Ruby, "Gemfile"),
        ] {
            let err = gen_with(runtime, &python, custom("make")).unwrap_err().to_string();
            assert_eq!(
                err,
                format!(
                    "runtime '{runtime}' was selected but the root directory has no {manifest} \
                     (it looks like a Python project: set the runtime to 'python' or 'auto')"
                )
            );
        }
        let err = gen_with(Runtime::Python, &[("Makefile", "")], custom("make")).unwrap_err().to_string();
        assert!(err.contains("has no requirements.txt"), "{err}");
        let err = gen_with(Runtime::Rust, &[("Makefile", "")], custom("make")).unwrap_err().to_string();
        assert_eq!(err, "runtime 'rust' was selected but the root directory has no Cargo.toml");
        let err = gen_with(Runtime::Go, &[("Makefile", "")], custom("make")).unwrap_err().to_string();
        assert_eq!(err, "runtime 'go' was selected but the root directory has no go.mod");
        // A custom build of loose Go files (no module) is still possible.
        let df = gen_ok(Runtime::Go, &[("main.go", "package main")], custom("go build -o /out/app main.go"));
        assert!(df.contains("go build -o /out/app main.go"), "{df}");
        // Matching projects keep building with their own commands.
        assert!(gen_with(Runtime::Go, &[("go.mod", "module x\n"), ("Makefile", "")], custom("make")).is_ok());
        assert!(gen_with(Runtime::Rust, &[("Cargo.toml", "[package]\nname = \"x\"\n")], custom("make")).is_ok());
    }

    #[test]
    fn python_pyproject_apps_install_dependencies_only() {
        // `uv init` app: [project] without a build system, flat modules.
        let uv_app = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"flask\"]\n";
        let df = gen_ok(Runtime::Python, &[("pyproject.toml", uv_app), ("main.py", ""), ("db.py", "")], opts());
        assert!(!df.contains("pip install --no-cache-dir ."), "{df}");
        assert!(has_line(&df, "RUN pip install --no-cache-dir uv"), "{df}");
        assert!(has_line(&df, "COPY pyproject.toml ./"), "{df}");
        assert!(instruction(&df, "uv pip install").contains("uv pip install --system -r pyproject.toml"), "{df}");
        assert!(pos(&df, "uv pip install") < pos(&df, "COPY . ."), "{df}");

        // With uv.lock: the locked dependencies, without the project itself.
        let df = gen_ok(
            Runtime::Python,
            &[("pyproject.toml", uv_app), ("uv.lock", ""), ("main.py", ""), ("db.py", "")],
            opts(),
        );
        let step = instruction(&df, "uv export");
        assert!(step.contains("uv export --frozen --no-dev --no-emit-project --no-hashes"), "{step}");
        assert!(step.contains("uv pip install --system -r /tmp/ferry-requirements.txt"), "{step}");
        assert!(step.contains("--mount=type=cache,target=/root/.cache/uv"), "{step}");

        // A uv package (build system + src/<name>): the project is installed too.
        let uv_pkg =
            format!("{uv_app}[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n");
        let df = gen_ok(
            Runtime::Python,
            &[("pyproject.toml", &uv_pkg), ("uv.lock", ""), ("src/app/__init__.py", ""), ("main.py", "")],
            opts(),
        );
        let step = instruction(&df, "uv export");
        assert!(!step.contains("--no-emit-project"), "{step}");

        // Poetry in non-package mode, or whose name matches no directory.
        for pyproject in [
            "[tool.poetry]\nname = \"svc\"\npackage-mode = false\n[build-system]\nrequires = [\"poetry-core\"]\n",
            "[tool.poetry]\nname = \"my-service\"\n[build-system]\nrequires = [\"poetry-core\"]\n",
        ] {
            let df = gen_ok(
                Runtime::Python,
                &[("pyproject.toml", pyproject), ("poetry.lock", ""), ("main.py", ""), ("db.py", "")],
                opts(),
            );
            assert!(has_line(&df, "RUN pip install --no-cache-dir poetry"), "{df}");
            let step = instruction(&df, "poetry install");
            assert!(
                step.contains(
                    "POETRY_VIRTUALENVS_CREATE=false poetry install --only main --no-interaction --no-ansi --no-root"
                ),
                "{step}"
            );
        }
        // A real Poetry package keeps the root install.
        let df = gen_ok(
            Runtime::Python,
            &[
                (
                    "pyproject.toml",
                    "[tool.poetry]\nname = \"my-service\"\n[build-system]\nrequires = [\"poetry-core\"]\n",
                ),
                ("my_service/__init__.py", ""),
                ("main.py", ""),
            ],
            opts(),
        );
        assert!(!instruction(&df, "poetry install").contains("--no-root"), "{df}");

        // A declared, discoverable package without a lock file: pip install .
        let df = gen_ok(
            Runtime::Python,
            &[
                ("pyproject.toml", "[build-system]\nrequires = [\"setuptools\"]\n[project]\nname = \"app\"\n"),
                ("app/__init__.py", ""),
                ("main.py", ""),
            ],
            opts(),
        );
        assert!(has_line(&df, "RUN pip install --no-cache-dir ."), "{df}");
        assert!(pos(&df, "COPY . .") < pos(&df, "RUN pip install"), "{df}");
        // ...but a build system over a flat layout it cannot package is not.
        let df = gen_ok(
            Runtime::Python,
            &[
                ("pyproject.toml", "[build-system]\nrequires = [\"setuptools\"]\n[project]\nname = \"app\"\n"),
                ("main.py", ""),
                ("db.py", ""),
            ],
            opts(),
        );
        assert!(!df.contains("pip install --no-cache-dir ."), "{df}");
        assert!(df.contains("uv pip install --system -r pyproject.toml"), "{df}");
    }

    #[test]
    fn database_drivers_get_their_system_libraries() {
        let apt = |files: &[(&str, &str)]| -> String {
            let df = gen_ok(Runtime::Python, files, opts());
            df.lines().find(|l| l.contains("apt-get install")).unwrap_or_default().to_string()
        };
        let line = apt(&[("requirements.txt", "Django\npsycopg2==2.9.9\n"), ("app.py", "")]);
        assert!(line.contains("build-essential") && line.contains("libpq-dev"), "{line}");
        let line = apt(&[("requirements.txt", "psycopg[pool]>=3.1\n"), ("app.py", "")]);
        assert!(line.contains("libpq5") && !line.contains("build-essential"), "{line}");
        let line = apt(&[
            ("requirements.txt", "-r requirements/base.txt\n"),
            ("requirements/base.txt", "mysqlclient\n"),
            ("app.py", ""),
        ]);
        assert!(line.contains("default-libmysqlclient-dev") && line.contains("pkg-config"), "{line}");
        let line =
            apt(&[("pyproject.toml", "[project]\nname = \"a\"\ndependencies = [\"psycopg[c]\"]\n"), ("main.py", "")]);
        assert!(line.contains("libpq-dev"), "{line}");
        // Binary wheels bundle libpq: nothing to install.
        assert_eq!(apt(&[("requirements.txt", "psycopg2-binary\npsycopg[binary]\n"), ("app.py", "")]), "");
        // The system step comes before (and without) the build-time env.
        let df = gen_ok(
            Runtime::Python,
            &[("requirements.txt", "psycopg2\n"), ("app.py", "")],
            DockerfileOptions { build_arg_keys: vec!["TOKEN".into()], ..opts() },
        );
        assert!(!instruction(&df, "apt-get install").contains("type=secret"), "{df}");
        assert!(pos(&df, "apt-get install") < pos(&df, "pip install"), "{df}");

        // Ruby always gets libpq-dev (the pg gem), mysql2 its client library.
        let df = gen_ok(Runtime::Ruby, &[("Gemfile", "gem 'pg'\n"), ("config.ru", "")], opts());
        assert!(instruction(&df, "apt-get install").contains("libpq-dev"), "{df}");
        assert!(!df.contains("default-libmysqlclient-dev"), "{df}");
        let df = gen_ok(Runtime::Ruby, &[("Gemfile", "gem 'mysql2'\n"), ("config.ru", "")], opts());
        assert!(instruction(&df, "apt-get install").contains("default-libmysqlclient-dev"), "{df}");
    }

    #[test]
    fn go_builds_the_main_package() {
        // A library at the root with the binary under cmd/: build cmd/<name>.
        let df = gen_ok(
            Runtime::Go,
            &[("go.mod", "module x\n"), ("lib.go", "package mylib\n"), ("cmd/server/main.go", "package main\n")],
            opts(),
        );
        assert!(df.contains("-o /out/app ./cmd/server"), "{df}");
        // A root tools.go does not make the root a main package.
        let df = gen_ok(
            Runtime::Go,
            &[
                ("go.mod", "module x\n"),
                ("tools.go", "//go:build tools\n\npackage tools\n"),
                ("cmd/api/main.go", "package main\n"),
                ("cmd/docs/README.md", ""),
            ],
            opts(),
        );
        assert!(df.contains("-o /out/app ./cmd/api"), "{df}");
        // Ambiguous or missing: a clear error instead of a broken image.
        let err = gen_with(
            Runtime::Go,
            &[("go.mod", "module x\n"), ("cmd/api/main.go", "package main"), ("cmd/worker/main.go", "package main")],
            opts(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("several commands under cmd/ (api, worker)"), "{err}");
        let err = gen_with(Runtime::Go, &[("go.mod", "module x\n"), ("lib.go", "package mylib")], opts())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not `package main`") && err.contains("set a build command"), "{err}");
        // An explicit build command is used as-is.
        let custom = DockerfileOptions { build_command: Some("go build -o /out/app ./cmd/api".into()), ..opts() };
        assert!(gen_with(Runtime::Go, &[("go.mod", "module x\n"), ("lib.go", "package mylib")], custom).is_ok());
    }

    #[test]
    fn static_sites_build_with_their_own_toolchain() {
        let site = |runtime: Runtime, files: &[(&str, &str)], build: &str| -> String {
            gen_ok(
                runtime,
                files,
                DockerfileOptions {
                    service_type: Some(ServiceType::StaticSite),
                    build_command: Some(build.to_string()),
                    build_arg_keys: vec!["SITE_URL".into()],
                    ..opts()
                },
            )
        };
        // MkDocs: a Python build stage with the requirements installed.
        let df = site(Runtime::Auto, &[("requirements.txt", "mkdocs-material\n"), ("mkdocs.yml", "")], "mkdocs build");
        assert!(has_line(&df, "FROM python:3.12-slim AS build"), "{df}");
        assert!(
            instruction(&df, "pip install --no-cache-dir -r requirements.txt").contains(&secret("SITE_URL")),
            "{df}"
        );
        assert!(instruction(&df, "mkdocs build").contains(&secret("SITE_URL")), "{df}");
        assert!(!df.contains("node:"), "{df}");
        assert!(pos(&df, "mkdocs build") < pos(&df, "FROM nginx:alpine"), "{df}");
        // Also with the runtime set to static explicitly.
        let df = site(Runtime::Static, &[("requirements.txt", "mkdocs\n")], "mkdocs build");
        assert!(has_line(&df, "FROM python:3.12-slim AS build"), "{df}");

        // Jekyll: a Ruby stage with bundle install.
        let df = site(
            Runtime::Auto,
            &[("Gemfile", "gem 'jekyll'\n"), ("Gemfile.lock", ""), ("_config.yml", "")],
            "bundle exec jekyll build",
        );
        assert!(has_line(&df, "FROM ruby:3.3-slim AS build"), "{df}");
        assert!(instruction(&df, "bundle install").starts_with("RUN "), "{df}");
        assert!(pos(&df, "bundle install") < pos(&df, "bundle exec jekyll build"), "{df}");

        // Hugo: by config file or by build command.
        for files in [&[("hugo.toml", "baseURL = '/'")][..], &[("config.toml", ""), ("content/_index.md", "")][..]] {
            let df = site(Runtime::Auto, files, "hugo --minify");
            assert!(has_line(&df, "FROM alpine:3.20 AS build"), "{df}");
            assert!(has_line(&df, "RUN apk add --no-cache git hugo"), "{df}");
            assert!(instruction(&df, "hugo --minify").contains(&secret("SITE_URL")), "{df}");
        }
        // Hugo with an npm toolchain (PostCSS): the Node stage gets Hugo too.
        let df = site(Runtime::Auto, &[("package.json", "{}"), ("hugo.yaml", "")], "npm ci && hugo");
        assert!(has_line(&df, "FROM node:22-alpine AS build"), "{df}");
        assert!(has_line(&df, "RUN apk add --no-cache git hugo"), "{df}");
        // Hugo modules (go.mod) also need Go.
        let df = site(Runtime::Auto, &[("go.mod", "module site\n"), ("hugo.toml", "")], "hugo");
        assert!(has_line(&df, "RUN apk add --no-cache git hugo go"), "{df}");

        // A Go program generating the site.
        let df = site(Runtime::Auto, &[("go.mod", "module gen\n"), ("main.go", "package main")], "go run . -out dist");
        assert!(has_line(&df, "FROM golang:1.23-alpine AS build"), "{df}");
        assert!(instruction(&df, "go run . -out dist").contains("--mount=type=cache,target=/go/pkg/mod"), "{df}");

        // Nothing recognizable: a plain Node image, as before.
        let df = site(Runtime::Auto, &[("build.sh", "")], "sh build.sh");
        assert!(has_line(&df, "FROM node:22-alpine AS build"), "{df}");
    }
}
