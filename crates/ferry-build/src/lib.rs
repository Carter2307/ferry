//! # ferry-build
//!
//! Turns source code into a Docker image:
//! 1. **Fetch** — clone/fetch a git repo (https, ssh, or local path/`file://`)
//!    at a branch or exact commit, or extract an uploaded `.tar.gz`.
//! 2. **Detect** — pick a runtime (Dockerfile → docker, package.json → node, ...).
//! 3. **Generate** — for native runtimes, write a Dockerfile (Nixpacks-style).
//! 4. **Build** — `docker build` (BuildKit, `--progress=plain`) streaming
//!    every output line to the [`LogSink`].
//!
//! Uses the `git` and `docker` CLIs (via `tokio::process`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ferry_core::{CancellationToken, LogSink, Result, Runtime, ServiceType};

/// Where the code comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSource {
    /// Clone `repo_url` at `branch`, or at `commit` when given.
    Git { repo_url: String, branch: String, commit: Option<String> },
    /// Extract a gzipped tarball (uploaded via `ferry up`).
    Archive { path: PathBuf },
}

/// Everything the builder needs for one build.
#[derive(Debug, Clone)]
pub struct BuildRequest {
    pub service_id: String,
    pub deploy_id: String,
    pub service_name: String,
    pub service_type: ServiceType,
    pub source: BuildSource,
    /// `Auto` = detect. Never `Image` (the engine pulls images itself).
    pub runtime: Runtime,
    /// Subdirectory used as build context.
    pub root_dir: Option<String>,
    /// Dockerfile path relative to the context (default `Dockerfile`).
    pub dockerfile_path: Option<String>,
    pub build_command: Option<String>,
    /// Baked into generated Dockerfiles as CMD. (For docker runtime the engine
    /// applies it at run time instead.)
    pub start_command: Option<String>,
    /// Static sites: directory to serve (default: auto-detect dist/build/public/out, else context root).
    pub publish_dir: Option<String>,
    /// Full image reference to produce, e.g. `ferry/web:dep-…`.
    pub image_tag: String,
    /// Env vars available at build time: `--build-arg` for user Dockerfiles,
    /// declared as `ARG` in generated ones.
    pub build_args: Vec<(String, String)>,
    /// Labels added to the image (`--label`).
    pub labels: BTreeMap<String, String>,
    /// `--no-cache`.
    pub clear_cache: bool,
}

/// Result of a successful build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutput {
    pub image: String,
    /// Resolved commit (git sources).
    pub commit_sha: Option<String>,
    /// First line of the commit message (git sources).
    pub commit_message: Option<String>,
    /// Runtime actually used (after detection).
    pub runtime: Runtime,
    /// Port the image is known to listen on (e.g. 80 for static sites on nginx).
    pub port_hint: Option<u16>,
}

/// Options for [`generate_dockerfile`].
#[derive(Debug, Clone, Default)]
pub struct DockerfileOptions {
    pub service_type: Option<ServiceType>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub publish_dir: Option<String>,
    /// Build-arg names to declare with `ARG`.
    pub build_arg_keys: Vec<String>,
}

/// A generated Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedDockerfile {
    pub contents: String,
    pub port_hint: Option<u16>,
}

/// Builds images. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Builder {
    builds_dir: PathBuf,
    repos_dir: PathBuf,
    docker_bin: String,
}

impl Builder {
    /// `builds_dir`: scratch contexts (`<builds_dir>/<deploy_id>`), removed
    /// after each build. `repos_dir`: persistent git caches
    /// (`<repos_dir>/<service_id>`) for fast incremental fetches.
    pub fn new(builds_dir: PathBuf, repos_dir: PathBuf, docker_bin: String) -> Self {
        Builder { builds_dir, repos_dir, docker_bin }
    }

    /// Fetch, detect, generate and build. Log lines: `==> ...` system
    /// messages for each step, then raw git/docker output. Honors `cancel`
    /// (kills child processes, returns `Error::Canceled`). Build failures
    /// return `Error::Build` with a concise reason (the full output is in
    /// the logs). Always cleans up the scratch directory.
    pub async fn build(&self, req: &BuildRequest, logs: &LogSink, cancel: &CancellationToken) -> Result<BuildOutput> {
        let _ = (req, logs, cancel, &self.builds_dir, &self.repos_dir, &self.docker_bin);
        todo!("ferry-build: build")
    }

    /// Resolve the commit sha a branch currently points to (`git ls-remote`).
    pub async fn resolve_branch_head(&self, repo_url: &str, branch: &str) -> Result<String> {
        let _ = (repo_url, branch);
        todo!("ferry-build: resolve_branch_head")
    }

    /// Delete the git cache of a service (on service deletion).
    pub async fn remove_repo_cache(&self, service_id: &str) -> Result<()> {
        let _ = service_id;
        todo!("ferry-build: remove_repo_cache")
    }
}

/// Detect the runtime of a source directory. Order: Dockerfile → docker;
/// package.json → node; requirements.txt/pyproject.toml/Pipfile → python;
/// go.mod → go; Cargo.toml → rust; Gemfile → ruby; index.html → static.
pub fn detect_runtime(dir: &Path) -> Option<Runtime> {
    let _ = dir;
    todo!("ferry-build: detect_runtime")
}

/// Generate a Dockerfile for a native runtime (`Node`, `Python`, `Go`,
/// `Rust`, `Ruby`, `Static`). Static sites (service type `StaticSite`, or
/// runtime `Static`) are served by `nginx:alpine` on port 80, after an
/// optional node build stage when `package.json` exists.
pub fn generate_dockerfile(runtime: Runtime, dir: &Path, opts: &DockerfileOptions) -> Result<GeneratedDockerfile> {
    let _ = (runtime, dir, opts);
    todo!("ferry-build: generate_dockerfile")
}
