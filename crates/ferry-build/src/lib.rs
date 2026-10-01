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

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use ferry_core::{CancellationToken, Error, LogSink, Result, Runtime, ServiceType};

mod archive;
mod buildenv;
mod detect;
mod docker;
mod dockerfile;
mod fsutil;
mod git;
mod orphans;
mod pm_errors;
mod process;
mod redact;

use crate::git::GitError;

pub use crate::git::Credentials as GitCredentials;
pub use crate::git::RemoteBranches;

/// Why a remote's branches could not be listed. Messages never contain
/// credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// The URL isn't one git can be given.
    Invalid(String),
    /// The remote refused, wasn't found or couldn't be reached in time.
    Unreachable(String),
}

/// The branches of a repository and its default one, asked from the remote
/// itself (`git ls-remote`): works for any URL a service can deploy from.
/// `credentials` authenticate an http(s) remote as in [`BuildSource::Git`];
/// credentials in the URL work too.
pub async fn remote_branches(
    repo_url: &str,
    credentials: Option<&GitCredentials>,
    timeout: std::time::Duration,
) -> std::result::Result<RemoteBranches, RemoteError> {
    git::list_remote_branches(repo_url, credentials, timeout).await.map_err(|e| match e {
        GitError::Invalid(m) => RemoteError::Invalid(m),
        GitError::NotFound(m) | GitError::Failed(m) => RemoteError::Unreachable(m),
        GitError::Canceled => RemoteError::Unreachable("canceled".into()),
    })
}

/// Where the code comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildSource {
    /// Clone `repo_url` at `branch`, or at `commit` when given.
    ///
    /// `credentials` authenticate an http(s) remote without being part of
    /// its URL (the token of a connected GitHub / GitLab account). Git gets
    /// them through its environment, for the remote's own host only; they
    /// never appear on a command line, in the git cache or in the logs.
    /// Ignored for ssh and local repositories.
    Git { repo_url: String, branch: String, commit: Option<String>, credentials: Option<GitCredentials> },
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
    /// An explicit value always wins, `.` included (the root directory, even
    /// when build output directories exist).
    pub publish_dir: Option<String>,
    /// Full image reference to produce, e.g. `ferry/web:dep-…`.
    pub image_tag: String,
    /// Env vars available at build time. Always passed as BuildKit secrets
    /// (`--secret id=KEY`, values through the docker CLI's environment, never
    /// its command line): generated Dockerfiles mount them into the steps
    /// that run project code (`RUN --mount=type=secret,id=KEY,env=KEY`) and
    /// never declare them as `ARG`, so values stay out of the image history.
    /// User Dockerfiles additionally get `--build-arg KEY`, so the ARGs they
    /// declare keep working.
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

/// Progress events emitted by [`Builder::build_with_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildEvent {
    /// The git source was checked out, before the image build starts (so a
    /// failed or canceled build can still record which commit it was).
    CheckedOut { commit_sha: String, commit_message: Option<String> },
}

/// Options for [`generate_dockerfile`].
#[derive(Debug, Clone, Default)]
pub struct DockerfileOptions {
    pub service_type: Option<ServiceType>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub publish_dir: Option<String>,
    /// Build-time variable names (see [`BuildRequest::build_args`]): exposed
    /// to install/build steps through BuildKit secret mounts, never `ARG`.
    pub build_arg_keys: Vec<String>,
}

/// A generated Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedDockerfile {
    pub contents: String,
    pub port_hint: Option<u16>,
    /// The command the image starts (`None`: static sites run nginx, and a
    /// cron job may get its command at run time).
    pub start: Option<StartCommand>,
}

/// The start command of a generated image, for the build log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartCommand {
    /// As it would be typed: `npm start`, `node ./dist/index.js`.
    pub command: String,
    /// Where it comes from: `the service's start command`, `the Procfile`,
    /// `the "start" script of package.json`…
    pub source: String,
}

impl StartCommand {
    /// The line of the build log: a command nobody chose is the first thing
    /// to look at when an instance doesn't stay up.
    pub fn log_line(&self) -> String {
        format!("==> Start command: {} ({})", self.command, self.source)
    }
}

/// Name of the Dockerfile the builder writes for native runtimes.
pub const GENERATED_DOCKERFILE: &str = "Dockerfile.ferry";

/// Default cap on the total size of an uploaded source archive's files once
/// extracted (see [`Builder::with_upload_limits`]).
pub const DEFAULT_MAX_UPLOAD_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default cap on the number of entries (files, directories, links) of an
/// uploaded source archive (see [`Builder::with_upload_limits`]).
pub const DEFAULT_MAX_UPLOAD_ENTRIES: u64 = 200_000;

/// `.dockerignore` written next to generated Dockerfiles when the project
/// has none.
const DEFAULT_DOCKERIGNORE: &str = "\
# Written by Ferry (no .dockerignore in the project)
.git
**/node_modules
target
.venv
**/__pycache__
Dockerfile.ferry*
.dockerignore
";

type RepoLocks = Arc<StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

/// Builds images. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Builder {
    builds_dir: PathBuf,
    repos_dir: PathBuf,
    docker_bin: String,
    /// Serializes git operations on one service's cache.
    repo_locks: RepoLocks,
    /// Keys the cache digest of the build-time env.
    env_key: buildenv::EnvKey,
    /// Decompression limits of uploaded archives.
    upload_limits: archive::Limits,
}

/// What [`prepare_context`] decided.
struct Prepared {
    context: PathBuf,
    dockerfile: PathBuf,
    runtime: Runtime,
    port_hint: Option<u16>,
    messages: Vec<String>,
    /// The generated Dockerfile declares the env digest build arg.
    env_digest_arg: bool,
}

impl Builder {
    /// `builds_dir`: scratch contexts (`<builds_dir>/<deploy_id>`), removed
    /// after each build. `repos_dir`: persistent git caches
    /// (`<repos_dir>/<service_id>`) for fast incremental fetches.
    pub fn new(builds_dir: PathBuf, repos_dir: PathBuf, docker_bin: String) -> Self {
        let repos_dir = absolute(repos_dir);
        Builder {
            builds_dir: absolute(builds_dir),
            env_key: buildenv::EnvKey::new(&repos_dir),
            repos_dir,
            docker_bin,
            repo_locks: Arc::new(StdMutex::new(HashMap::new())),
            upload_limits: archive::Limits::upload(DEFAULT_MAX_UPLOAD_EXTRACTED_BYTES, DEFAULT_MAX_UPLOAD_ENTRIES),
        }
    }

    /// Limits for uploaded archives ([`BuildSource::Archive`]): the total
    /// size of their files once extracted, and their number of entries.
    /// Sizes are checked from the tar headers before anything is written, so
    /// a decompression bomb (a few MB of gzip expanding to many GB) fails the
    /// build with a clear error instead of filling the disk. Defaults:
    /// [`DEFAULT_MAX_UPLOAD_EXTRACTED_BYTES`] (2 GiB) and
    /// [`DEFAULT_MAX_UPLOAD_ENTRIES`] (200 000). Git sources keep larger
    /// fixed limits (16 GiB, 2 000 000 entries).
    pub fn with_upload_limits(mut self, max_extracted_bytes: u64, max_entries: u64) -> Self {
        self.upload_limits = archive::Limits::upload(max_extracted_bytes, max_entries);
        self
    }

    /// Fetch, detect, generate and build. Log lines: `==> ...` system
    /// messages for each step, then raw git/docker output. Honors `cancel`
    /// (kills child processes, returns `Error::Canceled`). Build failures
    /// return `Error::Build` with a concise reason (the full output is in
    /// the logs). Always cleans up the scratch directory.
    pub async fn build(&self, req: &BuildRequest, logs: &LogSink, cancel: &CancellationToken) -> Result<BuildOutput> {
        self.build_with_events(req, logs, cancel, None).await
    }

    /// Like [`Builder::build`], additionally sending [`BuildEvent`]s to
    /// `events` as the build progresses (send errors are ignored).
    pub async fn build_with_events(
        &self,
        req: &BuildRequest,
        logs: &LogSink,
        cancel: &CancellationToken,
        events: Option<&tokio::sync::mpsc::UnboundedSender<BuildEvent>>,
    ) -> Result<BuildOutput> {
        match self.build_scratch(req, logs, cancel, events).await {
            Ok(out) => {
                logs.system("==> Build successful 🎉");
                tracing::info!(service = %req.service_name, deploy = %req.deploy_id, image = %out.image, "build succeeded");
                Ok(out)
            }
            Err(Error::Canceled) => {
                logs.system("==> Build canceled");
                tracing::info!(service = %req.service_name, deploy = %req.deploy_id, "build canceled");
                Err(Error::Canceled)
            }
            Err(e) => {
                let e = into_build_error(e);
                let reason = match &e {
                    Error::Build(m) => m.clone(),
                    other => other.to_string(),
                };
                logs.system(format!("==> Build failed: {reason}"));
                tracing::info!(service = %req.service_name, deploy = %req.deploy_id, "build failed: {reason}");
                Err(e)
            }
        }
    }

    /// Validate the request, run the build in `<builds_dir>/<deploy_id>` and
    /// always remove that directory afterwards.
    async fn build_scratch(
        &self,
        req: &BuildRequest,
        logs: &LogSink,
        cancel: &CancellationToken,
        events: Option<&tokio::sync::mpsc::UnboundedSender<BuildEvent>>,
    ) -> Result<BuildOutput> {
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        fsutil::safe_component("deploy id", &req.deploy_id).map_err(Error::Build)?;
        fsutil::safe_component("service id", &req.service_id).map_err(Error::Build)?;
        let tag = req.image_tag.trim();
        if tag.is_empty() || tag.starts_with('-') || tag.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Error::Build(format!("invalid image tag '{}'", req.image_tag)));
        }
        // `<deploy_id>/src` holds the source (the build context lives in it),
        // `<deploy_id>/children` the records of running git / docker
        // processes (see `reap_orphans`), outside the context.
        let scratch = fsutil::ScratchDir::create(self.builds_dir.join(&req.deploy_id))
            .await
            .map_err(|e| Error::Build(format!("cannot create build directory: {e}")))?;
        let source_dir = scratch.path().join("src");
        let res = match tokio::fs::create_dir(&source_dir).await {
            Ok(()) => {
                let records = scratch.path().join(orphans::CHILDREN_DIR);
                process::record_children_in(records, self.build_in(req, logs, cancel, &source_dir, events)).await
            }
            Err(e) => Err(Error::Build(format!("cannot create build directory: {e}"))),
        };
        scratch.cleanup().await;
        res
    }

    async fn build_in(
        &self,
        req: &BuildRequest,
        logs: &LogSink,
        cancel: &CancellationToken,
        source_dir: &Path,
        events: Option<&tokio::sync::mpsc::UnboundedSender<BuildEvent>>,
    ) -> Result<BuildOutput> {
        // 1. Fetch the source into the scratch directory.
        let (export_root, commit_sha, commit_message) = match &req.source {
            BuildSource::Git { repo_url, branch, commit, credentials } => {
                let lock = self.repo_lock(&req.service_id);
                let _guard = tokio::select! {
                    g = lock.lock() => g,
                    _ = cancel.cancelled() => return Err(Error::Canceled),
                };
                let cache = self.repos_dir.join(&req.service_id);
                let source =
                    git::Source { repo_url, credentials: credentials.as_ref(), branch, commit: commit.as_deref() };
                let co = git::checkout(&cache, source, source_dir, logs, cancel).await.map_err(git_build_error)?;
                for s in &co.report_skipped {
                    logs.system(format!("==> Skipped {s}"));
                }
                if co.has_submodules {
                    logs.system("==> Note: git submodules are not fetched");
                }
                let subject = (!co.subject.is_empty()).then_some(co.subject);
                (source_dir.to_path_buf(), Some(co.sha), subject)
            }
            BuildSource::Archive { path } => {
                logs.system("==> Extracting uploaded source archive");
                let (archive_path, dest, token) = (path.clone(), source_dir.to_path_buf(), cancel.clone());
                let limits = self.upload_limits;
                let hints = RootHints {
                    root_dir: req.root_dir.clone(),
                    dockerfile_path: req.dockerfile_path.clone(),
                    publish_dir: req.publish_dir.clone(),
                };
                let res = tokio::task::spawn_blocking(move || -> std::result::Result<_, archive::ExtractError> {
                    let report = archive::extract_archive_file(&archive_path, &dest, limits, &token)?;
                    let root = choose_archive_root(&dest, &hints)?;
                    Ok((report, root))
                })
                .await
                .map_err(|e| Error::Build(format!("extraction task failed: {e}")))?;
                let (report, root) = match res {
                    Ok(v) => v,
                    Err(archive::ExtractError::Canceled) => return Err(Error::Canceled),
                    Err(e) => return Err(Error::Build(e.to_string())),
                };
                for s in &report.skipped {
                    logs.system(format!("==> Skipped {s}"));
                }
                logs.system(format!("==> Extracted {} files ({})", report.files, human_bytes(report.bytes)));
                (root, None, None)
            }
        };
        if let (Some(tx), Some(sha)) = (events, &commit_sha) {
            let _ = tx.send(BuildEvent::CheckedOut { commit_sha: sha.clone(), commit_message: commit_message.clone() });
        }
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }

        // 2. Context, runtime and Dockerfile (filesystem work → blocking pool).
        let build_args: Vec<(String, String)> = {
            let mut out: Vec<(String, String)> = Vec::new();
            let mut skipped: Vec<&str> = Vec::new();
            for (k, v) in &req.build_args {
                if out.iter().any(|(seen, _)| seen == k) {
                    continue;
                }
                if !dockerfile::usable_build_arg(k) || v.contains('\0') {
                    skipped.push(k);
                    continue;
                }
                out.push((k.clone(), v.clone()));
            }
            if !skipped.is_empty() {
                logs.system(format!(
                    "==> Not available at build time (reserved or invalid names): {}",
                    skipped.join(", ")
                ));
            }
            out
        };
        let prep_req = PrepareRequest {
            export_root,
            root_dir: req.root_dir.clone(),
            runtime: req.runtime,
            service_type: req.service_type,
            dockerfile_path: req.dockerfile_path.clone(),
            options: DockerfileOptions {
                service_type: Some(req.service_type),
                build_command: req.build_command.clone(),
                start_command: req.start_command.clone(),
                publish_dir: req.publish_dir.clone(),
                build_arg_keys: build_args.iter().map(|(k, _)| k.clone()).collect(),
            },
        };
        let prepared = tokio::task::spawn_blocking(move || prepare_context(&prep_req))
            .await
            .map_err(|e| Error::Build(format!("preparing the build failed: {e}")))?
            .map_err(Error::Build)?;
        for m in &prepared.messages {
            logs.system(m.clone());
        }
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }

        // 3. docker build.
        logs.system(format!("==> Building image {}", req.image_tag.trim()));
        let env_digest = match prepared.env_digest_arg && !build_args.is_empty() {
            true => Some(self.env_key.digest(&build_args).await),
            false => None,
        };
        let docker_host = docker::daemon_host().await;
        let build = docker::DockerBuild {
            docker_bin: &self.docker_bin,
            dockerfile: &prepared.dockerfile,
            context: &prepared.context,
            tag: req.image_tag.trim(),
            labels: &req.labels,
            env: &build_args,
            build_args: prepared.runtime == Runtime::Docker,
            env_digest: env_digest.as_deref(),
            docker_host: docker_host.as_deref(),
            no_cache: req.clear_cache,
        };
        docker::build(&build, logs, cancel).await?;

        Ok(BuildOutput {
            image: req.image_tag.trim().to_string(),
            commit_sha,
            commit_message,
            runtime: prepared.runtime,
            port_hint: prepared.port_hint,
        })
    }

    /// Resolve the commit sha a branch currently points to (`git ls-remote`).
    ///
    /// Errors: `Invalid` for a malformed URL / branch or a branch that does not
    /// exist ("branch 'x' not found in <url> (the default branch is 'y')" —
    /// the hint is best effort), `Internal` when the remote cannot be
    /// reached. Credentials are never included in messages.
    pub async fn resolve_branch_head(&self, repo_url: &str, branch: &str) -> Result<String> {
        git::ls_remote_branch(repo_url, branch.trim()).await.map_err(|e| match e {
            GitError::NotFound(m) | GitError::Invalid(m) => Error::Invalid(m),
            GitError::Canceled => Error::Canceled,
            GitError::Failed(m) => Error::Internal(m),
        })
    }

    /// Kill the `git` / `docker build` processes a previous server left
    /// running, and return how many process groups were killed.
    ///
    /// Build children run in their own process groups and are recorded in
    /// the build's scratch directory while they run
    /// (`<builds_dir>/<deploy_id>/children/<pid>`). A server stopped
    /// normally kills them; one killed with SIGKILL cannot (on macOS nothing
    /// stops them), so a `docker build` keeps running, re-parented to init.
    /// A recorded group is killed only if its leader is still the recorded
    /// process: same program, started no later than recorded, leading its
    /// own group, not a child of this process — a reused pid is left alone.
    /// Every record is removed.
    ///
    /// Call it once at startup, before any build starts and before stale
    /// scratch directories are deleted (they hold the records). Blocking
    /// (reads the scratch directories, runs `ps`); returns 0 on non-Unix
    /// systems or when `ps` is unavailable.
    pub fn reap_orphans(&self) -> usize {
        orphans::reap(&self.builds_dir)
    }

    /// Delete the git cache of a service (on service deletion).
    pub async fn remove_repo_cache(&self, service_id: &str) -> Result<()> {
        fsutil::safe_component("service id", service_id).map_err(Error::Invalid)?;
        let lock = self.repo_lock(service_id);
        {
            let _guard = lock.lock().await;
            fsutil::remove_dir_async(self.repos_dir.join(service_id))
                .await
                .map_err(|e| Error::Internal(format!("removing the git cache of {service_id}: {e}")))?;
        }
        if let Ok(mut locks) = self.repo_locks.lock()
            && locks.get(service_id).is_some_and(|l| Arc::strong_count(l) <= 2)
        {
            locks.remove(service_id);
        }
        Ok(())
    }

    fn repo_lock(&self, service_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.repo_locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(service_id.to_string()).or_default().clone()
    }
}

/// The service paths that tell how an uploaded archive is rooted.
#[derive(Debug, Clone, Default)]
struct RootHints {
    root_dir: Option<String>,
    dockerfile_path: Option<String>,
    publish_dir: Option<String>,
}

/// Root of an extracted archive. An archive holding a single directory is
/// either wrapped (`tar czf app.tgz app/`: that directory is the project) or
/// the project itself holds a single directory (`ferry up` never wraps:
/// e.g. only `public/`). The interpretation where the service's configured
/// paths exist wins — a root directory that exists in only one of them
/// decides, then the Dockerfile path and publish directory (weighing more
/// than a merely recognizable project); ties strip the wrapper.
fn choose_archive_root(extracted: &Path, hints: &RootHints) -> io::Result<PathBuf> {
    let Some(top) = archive::single_top_level_dir(extracted)? else {
        return Ok(extracted.to_path_buf());
    };
    let root_dir = hints.root_dir.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let context = |base: &Path| fsutil::resolve_root_dir(base, root_dir).ok();
    let as_is = match (context(&top), context(extracted)) {
        (Some(stripped), Some(as_is)) => root_score(&as_is, hints) > root_score(&stripped, hints),
        (None, Some(_)) => true,
        _ => false,
    };
    Ok(if as_is { extracted.to_path_buf() } else { top })
}

/// How well a build context matches the service: 2 per configured path
/// found in it, 1 when a runtime is detected there.
fn root_score(context: &Path, hints: &RootHints) -> u32 {
    let found = |p: &Option<String>, dir: bool| {
        let Some(rel) = p.as_deref().map(str::trim).filter(|p| !p.is_empty()) else { return false };
        let Ok(rel) = fsutil::normalize_relative("path", rel) else { return false };
        let full = context.join(rel);
        if dir { full.is_dir() } else { full.is_file() }
    };
    2 * u32::from(found(&hints.dockerfile_path, false))
        + 2 * u32::from(found(&hints.publish_dir, true))
        + u32::from(detect::detect(context).is_some())
}

fn absolute(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

fn git_build_error(e: GitError) -> Error {
    match e {
        GitError::Canceled => Error::Canceled,
        other => Error::Build(other.message()),
    }
}

/// Every non-cancel failure of a build surfaces as `Error::Build`.
fn into_build_error(e: Error) -> Error {
    match e {
        Error::Canceled => Error::Canceled,
        Error::Build(m) => Error::Build(m),
        Error::Invalid(m) | Error::Internal(m) | Error::Conflict(m) | Error::Docker(m) | Error::Unauthorized(m) => {
            Error::Build(m)
        }
        other => Error::Build(other.to_string()),
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

struct PrepareRequest {
    export_root: PathBuf,
    root_dir: Option<String>,
    runtime: Runtime,
    service_type: ServiceType,
    dockerfile_path: Option<String>,
    options: DockerfileOptions,
}

/// Resolve the context, choose the runtime and locate or write the
/// Dockerfile. Blocking (filesystem); errors are user-facing messages.
fn prepare_context(req: &PrepareRequest) -> std::result::Result<Prepared, String> {
    let export_root = fs::canonicalize(&req.export_root).map_err(|e| format!("source directory: {e}"))?;
    let context = fsutil::resolve_root_dir(&export_root, req.root_dir.as_deref())?;
    let mut messages = Vec::new();
    if context != export_root {
        let rel = context.strip_prefix(&export_root).unwrap_or(&context);
        messages.push(format!("==> Using root directory ./{}", rel.display()));
    }

    let dockerfile_rel = req.dockerfile_path.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let runtime = match req.runtime {
        Runtime::Image => {
            return Err("runtime 'image' is not built from source (set an image on the service instead)".into());
        }
        Runtime::Auto => {
            // A custom Dockerfile path means "use Docker"; the default name
            // only when that file exists.
            let custom_path = dockerfile_rel.filter(|p| display_rel(p) != "Dockerfile");
            let explicit_dockerfile = custom_path.is_some()
                || dockerfile_rel
                    .is_some_and(|p| locate_dockerfile(&context, &export_root, p).is_ok_and(|f| f.is_some()));
            if explicit_dockerfile {
                Runtime::Docker
            } else {
                match detect::detect(&context) {
                    Some(r) => {
                        if r != Runtime::Docker {
                            messages.push(format!("==> Detected {} runtime", runtime_label(r)));
                        }
                        r
                    }
                    None if req.service_type == ServiceType::StaticSite => Runtime::Static,
                    None => {
                        return Err("could not detect the runtime: add a Dockerfile, or a package.json, \
                                    requirements.txt, pyproject.toml, Pipfile, go.mod, Cargo.toml, Gemfile \
                                    or index.html, or set the runtime explicitly"
                            .into());
                    }
                }
            }
        }
        r => r,
    };

    if runtime == Runtime::Docker {
        let rel = dockerfile_rel.unwrap_or("Dockerfile");
        let dockerfile = locate_dockerfile(&context, &export_root, rel)?.ok_or_else(|| {
            format!("Dockerfile not found at ./{} (paths are relative to the root directory)", display_rel(rel))
        })?;
        messages.push(format!("==> Using Dockerfile at ./{}", display_rel(rel)));
        return Ok(Prepared { context, dockerfile, runtime, port_hint: None, messages, env_digest_arg: false });
    }

    let generated = generate_dockerfile(runtime, &context, &req.options).map_err(|e| match e {
        Error::Invalid(m) | Error::Build(m) => m,
        other => other.to_string(),
    })?;
    let is_static = runtime == Runtime::Static || req.service_type == ServiceType::StaticSite;
    let used_runtime = if is_static { Runtime::Static } else { runtime };

    write_dockerignore_if_missing(&context).map_err(|e| format!("writing .dockerignore: {e}"))?;
    let dockerfile = write_generated_dockerfile(&context, &generated.contents)
        .map_err(|e| format!("writing the generated Dockerfile: {e}"))?;
    let name = dockerfile.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    messages.push(format!("==> Generated {name} for the {} runtime", runtime_label(used_runtime)));
    if let Some(start) = &generated.start {
        messages.push(start.log_line());
    }
    let digest_line = format!("ARG {}", dockerfile::ENV_DIGEST_ARG);
    let env_digest_arg = generated.contents.lines().any(|l| l.trim() == digest_line);
    Ok(Prepared {
        context,
        dockerfile,
        runtime: used_runtime,
        port_hint: generated.port_hint,
        messages,
        env_digest_arg,
    })
}

fn runtime_label(r: Runtime) -> &'static str {
    match r {
        Runtime::Node => "Node.js",
        Runtime::Python => "Python",
        Runtime::Go => "Go",
        Runtime::Rust => "Rust",
        Runtime::Ruby => "Ruby",
        Runtime::Static => "static site",
        Runtime::Docker => "Docker",
        Runtime::Image => "image",
        Runtime::Auto => "auto",
    }
}

fn display_rel(p: &str) -> String {
    p.trim_start_matches("./").to_string()
}

/// A user Dockerfile relative to the context; it may live elsewhere in the
/// checkout (`../Dockerfile`) but never outside it. `Ok(None)` when missing.
fn locate_dockerfile(context: &Path, export_root: &Path, rel: &str) -> std::result::Result<Option<PathBuf>, String> {
    if Path::new(rel).is_absolute() {
        return Err(format!("Dockerfile path '{rel}' must be relative to the root directory"));
    }
    match fsutil::resolve_inside(context, Path::new(rel), export_root).map_err(|e| format!("Dockerfile path {e}"))? {
        Some(p) if p.is_file() => Ok(Some(p)),
        Some(_) => Err(format!("Dockerfile path '{rel}' is not a file")),
        None => Ok(None),
    }
}

fn write_dockerignore_if_missing(context: &Path) -> io::Result<()> {
    let path = context.join(".dockerignore");
    if fs::symlink_metadata(&path).is_ok() {
        return Ok(());
    }
    match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => f.write_all(DEFAULT_DOCKERIGNORE.as_bytes()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Write `Dockerfile.ferry` without ever overwriting a project file: if the
/// name is taken, use `Dockerfile.ferry.1`, `.2`, …
fn write_generated_dockerfile(context: &Path, contents: &str) -> io::Result<PathBuf> {
    for i in 0..100 {
        let name = if i == 0 { GENERATED_DOCKERFILE.to_string() } else { format!("{GENERATED_DOCKERFILE}.{i}") };
        let path = context.join(&name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(contents.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free name for the generated Dockerfile"))
}

/// Detect the runtime of a source directory. Order: Dockerfile → docker;
/// package.json → node; requirements.txt/pyproject.toml/Pipfile → python;
/// go.mod → go; Cargo.toml → rust; Gemfile → ruby; index.html → static.
pub fn detect_runtime(dir: &Path) -> Option<Runtime> {
    detect::detect(dir)
}

/// Generate a Dockerfile for a native runtime (`Node`, `Python`, `Go`,
/// `Rust`, `Ruby`, `Static`). Static sites (service type `StaticSite`, or
/// runtime `Static`) are served by `nginx:alpine` on port 80, after a build
/// stage when there is a build command (or a `build` script): Node for
/// package.json, else the toolchain the project uses (Python for MkDocs,
/// Ruby for Jekyll, Hugo, Go).
pub fn generate_dockerfile(runtime: Runtime, dir: &Path, opts: &DockerfileOptions) -> Result<GeneratedDockerfile> {
    dockerfile::generate(runtime, dir, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prep(
        files: &[(&str, &str)],
        f: impl FnOnce(&mut PrepareRequest),
    ) -> (tempfile::TempDir, std::result::Result<Prepared, String>) {
        let d = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let p = d.path().join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }
        let mut req = PrepareRequest {
            export_root: d.path().to_path_buf(),
            root_dir: None,
            runtime: Runtime::Auto,
            service_type: ServiceType::WebService,
            dockerfile_path: None,
            options: DockerfileOptions { service_type: Some(ServiceType::WebService), ..Default::default() },
        };
        f(&mut req);
        let res = prepare_context(&req);
        (d, res)
    }

    #[test]
    fn prepare_uses_user_dockerfile() {
        let (_d, res) = prep(&[("Dockerfile", "FROM busybox"), ("package.json", "{}")], |_| {});
        let p = res.unwrap();
        assert_eq!(p.runtime, Runtime::Docker);
        assert!(p.dockerfile.ends_with("Dockerfile"));
        assert_eq!(p.port_hint, None);
        assert!(p.messages.iter().any(|m| m == "==> Using Dockerfile at ./Dockerfile"));
        assert!(!p.context.join(".dockerignore").exists());
    }

    #[test]
    fn prepare_custom_dockerfile_path_and_root_dir() {
        let (_d, res) = prep(
            &[("services/api/docker/Prod.Dockerfile", "FROM busybox"), ("services/api/package.json", "{}")],
            |r| {
                r.root_dir = Some("services/api".into());
                r.dockerfile_path = Some("./docker/Prod.Dockerfile".into());
            },
        );
        let p = res.unwrap();
        assert_eq!(p.runtime, Runtime::Docker);
        assert!(p.context.ends_with("services/api"));
        assert!(p.messages.iter().any(|m| m.contains("root directory ./services/api")));

        // explicit docker runtime with a missing Dockerfile → clear error
        let (_d, res) = prep(&[("index.html", "")], |r| r.runtime = Runtime::Docker);
        let err = res.err().unwrap();
        assert!(err.contains("Dockerfile not found at ./Dockerfile"), "{err}");

        // Dockerfile outside the checkout is refused
        let (_d, res) = prep(&[("index.html", "")], |r| {
            r.runtime = Runtime::Docker;
            r.dockerfile_path = Some("../../../../etc/hosts".into());
        });
        assert!(res.is_err());
        let (_d, res) = prep(&[("index.html", "")], |r| {
            r.runtime = Runtime::Docker;
            r.dockerfile_path = Some("/etc/hosts".into());
        });
        assert!(res.err().unwrap().contains("relative"));
    }

    #[test]
    fn prepare_custom_dockerfile_path_with_auto_runtime() {
        // A custom path that does not exist is an error, not a silent fallback.
        let (_d, res) = prep(&[("package.json", "{}"), ("index.js", "")], |r| {
            r.dockerfile_path = Some("docker/Dockerfile.prod".into());
        });
        let err = res.err().unwrap();
        assert!(err.contains("Dockerfile not found at ./docker/Dockerfile.prod"), "{err}");
        // The default name falls back to detection when absent.
        let (_d, res) = prep(&[("package.json", "{}"), ("index.js", "")], |r| {
            r.dockerfile_path = Some("./Dockerfile".into());
        });
        assert_eq!(res.unwrap().runtime, Runtime::Node);
    }

    #[test]
    fn prepare_generates_and_never_overwrites() {
        let (d, res) = prep(
            &[
                ("package.json", r#"{"scripts":{"start":"node s.js"}}"#),
                ("Dockerfile.ferry", "user file"),
                (".dockerignore", "custom"),
            ],
            |_| {},
        );
        let p = res.unwrap();
        assert_eq!(p.runtime, Runtime::Node);
        assert!(p.dockerfile.ends_with("Dockerfile.ferry.1"), "{:?}", p.dockerfile);
        assert_eq!(fs::read_to_string(d.path().join("Dockerfile.ferry")).unwrap(), "user file");
        assert_eq!(fs::read_to_string(d.path().join(".dockerignore")).unwrap(), "custom");
        assert!(fs::read_to_string(&p.dockerfile).unwrap().contains("FROM node:22-alpine"));
        assert!(p.messages.iter().any(|m| m == "==> Detected Node.js runtime"));

        let (d, res) = prep(&[("index.html", "")], |_| {});
        let p = res.unwrap();
        assert_eq!(p.runtime, Runtime::Static);
        assert_eq!(p.port_hint, Some(80));
        let ignore = fs::read_to_string(d.path().join(".dockerignore")).unwrap();
        for entry in [".git", "node_modules", "target", ".venv", "__pycache__"] {
            assert!(ignore.contains(entry), "{ignore}");
        }
    }

    #[test]
    fn prepare_static_site_type() {
        // StaticSite with a node project: node build stage + nginx
        let (_d, res) = prep(&[("package.json", r#"{"scripts":{"build":"vite build"}}"#)], |r| {
            r.service_type = ServiceType::StaticSite;
            r.options.service_type = Some(ServiceType::StaticSite);
        });
        let p = res.unwrap();
        assert_eq!(p.runtime, Runtime::Static);
        assert_eq!(p.port_hint, Some(80));
        // StaticSite with nothing detectable but a publish dir
        let (_d, res) = prep(&[("site/index.html", "")], |r| {
            r.service_type = ServiceType::StaticSite;
            r.options.service_type = Some(ServiceType::StaticSite);
            r.options.publish_dir = Some("site".into());
        });
        assert_eq!(res.unwrap().runtime, Runtime::Static);
    }

    #[test]
    fn prepare_errors() {
        let (_d, res) = prep(&[("README.md", "")], |_| {});
        assert!(res.err().unwrap().contains("could not detect the runtime"));
        let (_d, res) = prep(&[("index.html", "")], |r| r.runtime = Runtime::Image);
        assert!(res.is_err());
        let (_d, res) = prep(&[("index.html", "")], |r| r.root_dir = Some("../..".into()));
        assert!(res.err().unwrap().contains("outside"));
        let (_d, res) = prep(&[("index.html", "")], |r| r.root_dir = Some("missing".into()));
        assert!(res.err().unwrap().contains("does not exist"));
    }

    #[test]
    fn archive_root_selection() {
        let root_dir = |r: &str| RootHints { root_dir: Some(r.into()), ..Default::default() };
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("proj/services/api")).unwrap();
        // single top-level dir → it becomes the root
        assert!(choose_archive_root(d.path(), &RootHints::default()).unwrap().ends_with("proj"));
        assert!(choose_archive_root(d.path(), &root_dir("services/api")).unwrap().ends_with("proj"));
        // root_dir only valid from the extraction dir → keep the extraction dir
        assert_eq!(choose_archive_root(d.path(), &root_dir("proj/services/api")).unwrap(), d.path());
        // root files → no stripping
        fs::write(d.path().join("README"), "x").unwrap();
        assert_eq!(choose_archive_root(d.path(), &RootHints::default()).unwrap(), d.path());
    }

    #[test]
    fn archive_root_keeps_a_projects_only_directory() {
        let publish = |p: &str| RootHints { publish_dir: Some(p.into()), ..Default::default() };
        // `ferry up --dir only --type static --publish-dir public` where
        // `only/` holds just `public/index.html`: the archive is rooted.
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("public")).unwrap();
        fs::write(d.path().join("public/index.html"), "hi").unwrap();
        assert_eq!(choose_archive_root(d.path(), &publish("public")).unwrap(), d.path());
        assert_eq!(choose_archive_root(d.path(), &publish("./public/")).unwrap(), d.path());
        // Without a publish dir, serving public/ as the root is equivalent.
        assert!(choose_archive_root(d.path(), &RootHints::default()).unwrap().ends_with("public"));

        // A custom Dockerfile path inside the only directory.
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("docker")).unwrap();
        fs::write(d.path().join("docker/Dockerfile"), "FROM busybox").unwrap();
        let hints = RootHints { dockerfile_path: Some("docker/Dockerfile".into()), ..Default::default() };
        assert_eq!(choose_archive_root(d.path(), &hints).unwrap(), d.path());

        // A wrapped archive still strips its wrapper, also when the publish
        // dir only appears after the build or lives inside the project.
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("site/public")).unwrap();
        fs::write(d.path().join("site/package.json"), "{}").unwrap();
        assert!(choose_archive_root(d.path(), &publish("dist")).unwrap().ends_with("site"));
        assert!(choose_archive_root(d.path(), &publish("public")).unwrap().ends_with("site"));
        let hints = RootHints { dockerfile_path: Some("Dockerfile".into()), ..Default::default() };
        assert!(choose_archive_root(d.path(), &hints).unwrap().ends_with("site"));
    }

    #[test]
    fn build_errors_are_normalized() {
        assert!(matches!(into_build_error(Error::Canceled), Error::Canceled));
        assert!(matches!(into_build_error(Error::invalid("x")), Error::Build(m) if m == "x"));
        let io = into_build_error(Error::Io(io::Error::other("disk full")));
        assert!(matches!(io, Error::Build(m) if m.contains("disk full")));
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
    }

    #[tokio::test]
    async fn rejects_bad_ids_and_tags() {
        let b = Builder::new(PathBuf::from("/tmp/ferry-none/b"), PathBuf::from("/tmp/ferry-none/r"), "docker".into());
        let mut req = BuildRequest {
            service_id: "srv-1".into(),
            deploy_id: "../evil".into(),
            service_name: "web".into(),
            service_type: ServiceType::WebService,
            source: BuildSource::Archive { path: "/nonexistent.tar.gz".into() },
            runtime: Runtime::Auto,
            root_dir: None,
            dockerfile_path: None,
            build_command: None,
            start_command: None,
            publish_dir: None,
            image_tag: "ferry/web:dep-1".into(),
            build_args: vec![],
            labels: BTreeMap::new(),
            clear_cache: false,
        };
        let (logs, _rx) = LogSink::channel();
        let cancel = CancellationToken::new();
        assert!(matches!(b.build(&req, &logs, &cancel).await, Err(Error::Build(_))));
        req.deploy_id = "dep-1".into();
        req.image_tag = "--evil".into();
        assert!(matches!(b.build(&req, &logs, &cancel).await, Err(Error::Build(_))));
        cancel.cancel();
        assert!(matches!(b.build(&req, &logs, &cancel).await, Err(Error::Canceled)));
        assert!(b.remove_repo_cache("../x").await.is_err());
    }

    #[tokio::test]
    async fn oversized_uploads_fail_with_a_clear_error() {
        use std::io::Read as _;
        let root = tempfile::tempdir().unwrap();
        // 3 MiB of zeros compress to a few KiB.
        let mut tar = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_path("index.html").unwrap();
        h.set_size(3 * 1024 * 1024);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append(&h, io::repeat(b' ').take(3 * 1024 * 1024)).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        let archive = root.path().join("up.tar.gz");
        fs::write(&archive, gz.finish().unwrap()).unwrap();

        let b = Builder::new(root.path().join("builds"), root.path().join("repos"), "docker".into());
        assert_eq!(b.upload_limits.max_bytes, DEFAULT_MAX_UPLOAD_EXTRACTED_BYTES);
        assert_eq!(b.upload_limits.max_entries, DEFAULT_MAX_UPLOAD_ENTRIES);
        let b = b.with_upload_limits(1024 * 1024, 1000);
        let req = BuildRequest {
            service_id: "srv-1".into(),
            deploy_id: "dep-bomb".into(),
            service_name: "web".into(),
            service_type: ServiceType::StaticSite,
            source: BuildSource::Archive { path: archive },
            runtime: Runtime::Auto,
            root_dir: None,
            dockerfile_path: None,
            build_command: None,
            start_command: None,
            publish_dir: None,
            image_tag: "ferry/web:dep-bomb".into(),
            build_args: vec![],
            labels: BTreeMap::new(),
            clear_cache: false,
        };
        let (logs, mut rx) = LogSink::channel();
        let err = b.build(&req, &logs, &CancellationToken::new()).await.unwrap_err();
        let expected = "the uploaded archive expands to more than 1.0 MiB (the limit for extracted sources): leave \
                        dependencies, build outputs and large data out of it (.gitignore / .ferryignore)";
        assert!(matches!(&err, Error::Build(m) if m == expected), "{err}");
        assert!(!root.path().join("builds/dep-bomb").exists());
        let mut lines = Vec::new();
        while let Ok(l) = rx.try_recv() {
            lines.push(l.line);
        }
        assert_eq!(lines.last().map(String::as_str), Some(format!("==> Build failed: {expected}").as_str()));
    }

    #[tokio::test]
    async fn missing_archive_fails_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let b = Builder::new(root.path().join("builds"), root.path().join("repos"), "docker".into());
        let req = BuildRequest {
            service_id: "srv-1".into(),
            deploy_id: "dep-1".into(),
            service_name: "web".into(),
            service_type: ServiceType::WebService,
            source: BuildSource::Archive { path: root.path().join("missing.tar.gz") },
            runtime: Runtime::Auto,
            root_dir: None,
            dockerfile_path: None,
            build_command: None,
            start_command: None,
            publish_dir: None,
            image_tag: "ferry/web:dep-1".into(),
            build_args: vec![],
            labels: BTreeMap::new(),
            clear_cache: false,
        };
        let (logs, mut rx) = LogSink::channel();
        let err = b.build(&req, &logs, &CancellationToken::new()).await.unwrap_err();
        assert!(matches!(&err, Error::Build(m) if m.contains("not found")), "{err}");
        assert!(!root.path().join("builds/dep-1").exists());
        let mut lines = Vec::new();
        while let Ok(l) = rx.try_recv() {
            lines.push(l.line);
        }
        assert!(lines.iter().any(|l| l.starts_with("==> Build failed")), "{lines:?}");
        b.remove_repo_cache("srv-1").await.unwrap();
    }
}
