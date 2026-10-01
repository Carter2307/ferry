//! One deploy, start to finish: build (or pull / reuse) → start instances →
//! health check → route swap → live → drain the old instances. On failure
//! the new instances are removed and the old ones keep serving.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ferry_build::{BuildEvent, BuildRequest, BuildSource, GitCredentials};
use ferry_core::{
    Deploy, DeploySource, DeployStatus, DeployTrigger, EnvVar, Error, GitConnection, LogLine, LogSink, Runtime,
    Service, ServiceType, env,
};
use ferry_docker::{ContainerInfo, Docker};
use futures::StreamExt;
use futures::future::try_join_all;
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::health::{HealthCheck, Probe, wait_healthy};
use crate::images;
use crate::instances::{self, BuildInfo, STOP_GRACE_SECS};
use crate::limits::{self, Resources};
use crate::logs::LogHandle;
use crate::spec::{self, LaunchSpec};
use crate::state::{ActiveDeploy, DeployOptions, Inner, SetGuard, deploying_set};
use crate::util::{error_message, instance_id, lock};

/// Pause between routing traffic to the new instances and stopping the old
/// ones, so requests already in flight on the old instances can finish.
const DRAIN_DELAY: Duration = Duration::from_secs(2);
/// Log lines of a crashed instance copied into the deploy log.
const CRASH_TAIL_LINES: usize = 50;
/// How often a deploy waiting for a referenced service looks again.
const REFERENCE_POLL: Duration = Duration::from_secs(2);
/// A deploy waits at most this long (or twice the health-check timeout, if
/// longer) for the first deploy of a service it references.
const REFERENCE_WAIT_MIN: Duration = Duration::from_secs(15 * 60);
/// Launch specs of recent non-live deploys are deleted after each deploy.
const SPEC_GC_WINDOW: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Build,
    Deploy,
}

/// Why a deploy did not go live.
#[derive(Debug)]
struct Failure {
    stage: Stage,
    error: Error,
    /// The failure line is already in the deploy log (the builder writes
    /// `==> Build failed: …` itself).
    logged: bool,
}

impl Failure {
    fn build(error: Error) -> Self {
        Failure { stage: Stage::Build, error, logged: false }
    }

    fn deploy(error: Error) -> Self {
        Failure { stage: Stage::Deploy, error, logged: false }
    }
}

/// What the build stage produced.
#[derive(Debug, Clone)]
struct Built {
    image: String,
    info: BuildInfo,
}

struct Ctx {
    inner: Arc<Inner>,
    deploy_id: String,
    service_id: String,
    active: ActiveDeploy,
    log: LogHandle,
    /// Resumes the (suspended) service once live (see `DeployOptions::resume`).
    resume: bool,
}

impl Ctx {
    fn cancel(&self) -> &CancellationToken {
        &self.active.cancel
    }

    fn check_cancel(&self, stage: Stage) -> Result<(), Failure> {
        if self.cancel().is_cancelled() {
            Err(Failure { stage, error: Error::Canceled, logged: false })
        } else {
            Ok(())
        }
    }

    fn canceled(&self, stage: Stage, reason: &str) -> Failure {
        self.active.request_cancel(reason);
        Failure { stage, error: Error::Canceled, logged: false }
    }
}

/// Run a claimed deploy (status `building`) to a terminal status.
pub(crate) async fn run(
    inner: Arc<Inner>,
    deploy: Deploy,
    active: ActiveDeploy,
    log: LogHandle,
    opts: DeployOptions,
    permit: Option<OwnedSemaphorePermit>,
) {
    let ctx = Ctx {
        inner,
        deploy_id: deploy.id.clone(),
        service_id: deploy.service_id.clone(),
        active,
        log,
        resume: opts.resume,
    };
    let result = execute(&ctx, deploy, opts, permit).await;
    if let Err(failure) = result {
        record_failure(&ctx, failure).await;
    }
}

async fn execute(
    ctx: &Ctx,
    deploy: Deploy,
    opts: DeployOptions,
    permit: Option<OwnedSemaphorePermit>,
) -> Result<(), Failure> {
    let inner = &ctx.inner;
    let svc = match inner.store.get_service(&ctx.service_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Err(ctx.canceled(Stage::Build, "service deleted")),
        Err(e) => return Err(Failure::build(e)),
    };
    ctx.log.system(start_line(&deploy));
    if ctx.resume {
        ctx.log.system(format!(
            "==> '{}' is suspended and the image of its live deploy no longer exists: it resumes when this deploy \
             is live (and stays suspended if it fails)",
            svc.name
        ));
    }
    info!(service = %svc.name, deploy = %deploy.id, "deploy started");

    let built = build_stage(ctx, &svc, &deploy, opts).await?;
    drop(permit);
    record_build(ctx, &deploy, &built).await.map_err(Failure::build)?;
    ctx.check_cancel(Stage::Build)?;

    // Services this one references by port must have deployed first.
    wait_for_references(ctx).await?;

    if svc.service_type == ServiceType::CronJob {
        // Nothing to start: freeze what the runs will use, and go live.
        let svc = current_service(ctx, Stage::Deploy).await?;
        let user_env = spec::resolve_user_env(inner, &svc).await.map_err(Failure::deploy)?;
        let deploy = inner.store.require_deploy(&ctx.deploy_id).await.map_err(Failure::deploy)?;
        let spec = LaunchSpec::new(&svc, &deploy, built.image, None, built.info.runtime, user_env, &inner.config);
        log_resources(ctx, &svc, &spec.resources(inner).await, "per run");
        mark_live(ctx, &spec).await.map_err(Failure::deploy)?;
        announce_live(ctx, None).await;
        return Ok(());
    }
    deploy_stage(ctx, built).await
}

fn start_line(deploy: &Deploy) -> String {
    match (&deploy.trigger, &deploy.source) {
        (DeployTrigger::Rollback, DeploySource::Reuse { from_deploy: Some(from), .. }) => {
            format!("==> Rolling back to deploy {from} (deploy {})", deploy.id)
        }
        (trigger, _) => format!("==> Starting deploy {} (trigger: {trigger})", deploy.id),
    }
}

/// The service as it is now (it may have been changed while building). A
/// deploy that resumes the service sees it as it will be once live (a
/// suspend during that deploy cancels it like any other).
async fn current_service(ctx: &Ctx, stage: Stage) -> Result<Service, Failure> {
    match ctx.inner.store.get_service(&ctx.service_id).await {
        Ok(Some(mut s)) if s.suspended && ctx.resume => {
            s.suspended = false;
            Ok(s)
        }
        Ok(Some(s)) if s.suspended => Err(ctx.canceled(stage, "service suspended")),
        Ok(Some(s)) => Ok(s),
        Ok(None) => Err(ctx.canceled(stage, "service deleted")),
        Err(e) => Err(Failure { stage, error: e, logged: false }),
    }
}

// ---------------------------------------------------------------------------
// build stage

async fn build_stage(ctx: &Ctx, svc: &Service, deploy: &Deploy, opts: DeployOptions) -> Result<Built, Failure> {
    let inner = &ctx.inner;
    // Builds and pulls fill the disk: fail early (with a clear message) when
    // it is nearly full. Reusing an image (restart, rollback) needs no space.
    if !matches!(deploy.source, DeploySource::Reuse { .. }) {
        limits::check_free_disk(inner).await.map_err(Failure::build)?;
    }
    match &deploy.source {
        DeploySource::Git { repo_url, branch, commit } => {
            let connection = git_connection(ctx, svc, repo_url).await.map_err(Failure::build)?;
            let credentials = connection
                .as_ref()
                .map(|c| GitCredentials { username: c.provider.git_username().to_string(), password: c.token.clone() });
            let source = BuildSource::Git {
                repo_url: repo_url.clone(),
                branch: branch.clone(),
                commit: commit.clone(),
                credentials,
            };
            let built = build_image(ctx, svc, deploy, source, opts).await;
            if let Err(Failure { error, .. }) = &built
                && let Some(hint) = git_access_hint(error, connection.as_ref(), repo_url)
            {
                ctx.log.system(hint);
            }
            built
        }
        DeploySource::Archive { path } => {
            build_image(ctx, svc, deploy, BuildSource::Archive { path: PathBuf::from(path) }, opts).await
        }
        DeploySource::Image { image } => {
            let image = image.trim().to_string();
            if image.is_empty() {
                return Err(Failure::build(Error::Build("the service has no image to deploy".into())));
            }
            pull(ctx, &image).await.map_err(Failure::build)?;
            // Pin what was pulled: the reference may move (`latest`, a re-pushed
            // tag), but rollbacks, restarts, crash replacements and jobs of this
            // deploy must run exactly this image. The pin is one of the service's
            // own images, so retention deletes only this tag, never the pulled image.
            let pinned = inner.naming.image_tag(&svc.name, &deploy.id);
            inner.docker.tag_image(&image, &pinned).await.map_err(|e| {
                Failure::build(Error::Build(format!("pinning image {image} as {pinned}: {}", error_message(&e))))
            })?;
            ctx.log.system(format!("==> Pinned {image} as {pinned}"));
            Ok(Built { image: pinned, info: BuildInfo { runtime: Some(Runtime::Image), port_hint: None } })
        }
        DeploySource::Reuse { image, from_deploy } => {
            let (image, from_deploy) =
                current_reuse_target(ctx, svc, deploy, image, from_deploy.as_deref()).await.map_err(Failure::build)?;
            let (image, from_deploy) = (&image, &from_deploy);
            let exists = inner.docker.image_exists(image).await.map_err(Failure::build)?;
            if !exists {
                // A restart reuses the live image (never deleted by retention).
                let live = crate::deploy::is_restart(deploy.trigger);
                let reason = images::missing_reason(inner, svc, image, live).await;
                let what = match (from_deploy, live) {
                    (Some(from), true) => format!("the image of the live deploy {from} ({image})"),
                    (Some(from), false) => format!("the image of deploy {from} ({image})"),
                    (None, _) => format!("image {image}"),
                };
                return Err(Failure::build(Error::Build(format!(
                    "{what} no longer exists: {reason} — deploy again from source"
                ))));
            }
            let info = match from_deploy {
                Some(from) => {
                    ctx.log.system(format!("==> Reusing image {image} of deploy {from}"));
                    instances::load_build_info(&inner.store, from).await
                }
                None => {
                    ctx.log.system(format!("==> Reusing image {image}"));
                    None
                }
            };
            Ok(Built { image: image.clone(), info: info.unwrap_or_default() })
        }
    }
}

/// The service's git connection, when it has one for `repo_url`: its token
/// authenticates the clone. A token is only ever used for a repository on
/// its own provider instance ([`GitConnection::serves`]).
async fn git_connection(ctx: &Ctx, svc: &Service, repo_url: &str) -> ferry_core::Result<Option<GitConnection>> {
    let Some(id) = svc.git_connection_id.as_deref() else { return Ok(None) };
    let Some(connection) = ctx.inner.store.get_git_connection(id).await? else {
        ctx.log.system("==> Warning: the git connection of this service no longer exists: cloning without it");
        return Ok(None);
    };
    if !connection.serves(repo_url) {
        ctx.log.system(format!(
            "==> Warning: the {} is connected to {}, which this repository is not on: cloning without it",
            connection.describe(),
            connection.base_url
        ));
        return Ok(None);
    }
    ctx.log.system(format!("==> Cloning with the {}", connection.describe()));
    Ok(Some(connection))
}

/// What to do about a clone the remote refused: a line for the deploy log,
/// or `None` when the failure doesn't look like an access problem.
fn git_access_hint(error: &Error, connection: Option<&GitConnection>, repo_url: &str) -> Option<String> {
    let Error::Build(message) = error else { return None };
    let m = message.to_ascii_lowercase();
    // Wrong credentials, none where some are needed (git can't prompt: which
    // of the two messages depends on its version), or a private repository
    // the remote won't admit to.
    let refused = m.contains("authentication failed")
        || m.contains("unable to get password")
        || m.contains("could not read username")
        || m.contains("could not read password")
        || m.contains("access denied")
        || m.contains("returned error: 403")
        || (m.contains("repository") && m.contains("not found"));
    if !refused {
        return None;
    }
    match connection {
        Some(c) => Some(format!(
            "==> Hint: the token of the {} may have expired, been revoked or lack access to this repository: \
             connect the account again with a new token",
            c.describe()
        )),
        // Credentials in the URL are the user's own business.
        None if ferry_core::git::parse_http_url(repo_url).is_some() => Some(
            "==> Hint: if the repository is private, connect its GitHub or GitLab account and select it in the \
             service's settings"
                .to_string(),
        ),
        None => None,
    }
}

/// Restarts reuse what is live when they run, not when they were queued
/// (else a restart queued behind another deploy would roll that deploy
/// back, and one queued behind a first deploy would have nothing to reuse):
/// re-point their source at the current live deploy. Rollbacks keep their
/// explicit target.
async fn current_reuse_target(
    ctx: &Ctx,
    svc: &Service,
    deploy: &Deploy,
    image: &str,
    from_deploy: Option<&str>,
) -> ferry_core::Result<(String, Option<String>)> {
    let unchanged = (image.to_string(), from_deploy.map(str::to_string));
    if !crate::deploy::is_restart(deploy.trigger) {
        return Ok(unchanged);
    }
    let inner = &ctx.inner;
    let Some(live_id) = svc.live_deploy_id.as_deref() else {
        return Err(Error::Build(format!(
            "service '{}' has no live deploy to restart (its deploy did not go live)",
            svc.name
        )));
    };
    let Some(live_image) = inner.store.get_deploy(live_id).await?.and_then(|d| d.image) else {
        return Err(Error::Build(format!("the live deploy {live_id} of '{}' has no image", svc.name)));
    };
    if from_deploy == Some(live_id) && image == live_image {
        return Ok(unchanged);
    }
    let mut d = inner.store.require_deploy(&deploy.id).await?;
    d.source = DeploySource::Reuse { image: live_image.clone(), from_deploy: Some(live_id.to_string()) };
    inner.store.update_deploy(&d).await?;
    if from_deploy != Some(live_id) {
        ctx.log.system(format!("==> The live deploy changed since this restart was queued: restarting {live_id}"));
    }
    Ok((live_image, Some(live_id.to_string())))
}

async fn build_image(
    ctx: &Ctx,
    svc: &Service,
    deploy: &Deploy,
    source: BuildSource,
    opts: DeployOptions,
) -> Result<Built, Failure> {
    let inner = &ctx.inner;
    let (build_args, skipped) = instances::build_args(inner, svc).await.map_err(Failure::build)?;
    if !skipped.is_empty() {
        ctx.log.system(format!(
            "==> Warning: not available at build time (unresolved references): {}",
            skipped.join(", ")
        ));
    }
    let req = BuildRequest {
        service_id: svc.id.clone(),
        deploy_id: deploy.id.clone(),
        service_name: svc.name.clone(),
        service_type: svc.service_type,
        source,
        runtime: if svc.runtime == Runtime::Image { Runtime::Auto } else { svc.runtime },
        root_dir: svc.root_dir.clone(),
        dockerfile_path: svc.dockerfile_path.clone(),
        build_command: svc.build_command.clone(),
        start_command: svc.start_command.clone(),
        publish_dir: svc.publish_dir.clone(),
        image_tag: inner.naming.image_tag(&svc.name, &deploy.id),
        build_args,
        labels: inner.naming.service_labels(&svc.id, &deploy.id),
        clear_cache: opts.clear_cache,
    };
    // The commit is recorded as soon as it is checked out, so failed,
    // canceled and in-progress builds show it too.
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let result = {
        let build = inner.builder.build_with_events(&req, ctx.log.sink(), ctx.cancel(), Some(&events_tx));
        tokio::pin!(build);
        loop {
            tokio::select! {
                r = &mut build => break r,
                Some(event) = events.recv() => on_build_event(ctx, event).await,
            }
        }
    };
    drop(events_tx);
    while let Ok(event) = events.try_recv() {
        on_build_event(ctx, event).await;
    }
    match result {
        Ok(out) => {
            if let Some(sha) = out.commit_sha.clone() {
                record_commit(ctx, sha, out.commit_message.clone()).await;
            }
            Ok(Built { image: out.image, info: BuildInfo { runtime: Some(out.runtime), port_hint: out.port_hint } })
        }
        Err(Error::Canceled) => Err(Failure::build(Error::Canceled)),
        // The builder already wrote "==> Build failed: <reason>".
        Err(e) => Err(Failure { stage: Stage::Build, error: e, logged: true }),
    }
}

async fn on_build_event(ctx: &Ctx, event: BuildEvent) {
    match event {
        BuildEvent::CheckedOut { commit_sha, commit_message } => record_commit(ctx, commit_sha, commit_message).await,
    }
}

/// Store the commit on the deploy (unless it is already there).
async fn record_commit(ctx: &Ctx, sha: String, message: Option<String>) {
    let store = &ctx.inner.store;
    let recorded = async {
        let mut d = store.require_deploy(&ctx.deploy_id).await?;
        if d.commit_sha.as_deref() == Some(sha.as_str()) && d.commit_message == message {
            return Ok(());
        }
        d.commit_sha = Some(sha);
        d.commit_message = message;
        store.update_deploy(&d).await
    }
    .await;
    if let Err(e) = recorded {
        warn!(deploy = %ctx.deploy_id, "cannot record the commit: {e}");
    }
}

/// Image sources: pull untagged/`latest` images every time, others only
/// when missing. A failed pull falls back to a local copy if there is one.
async fn pull(ctx: &Ctx, image: &str) -> ferry_core::Result<()> {
    let docker = &ctx.inner.docker;
    if !images::always_pull(image) && docker.image_exists(image).await? {
        ctx.log.system(format!("==> Using image {image} (already present)"));
        return Ok(());
    }
    ctx.log.system(format!("==> Pulling image {image}"));
    let res = tokio::select! {
        r = docker.pull_image(image, ctx.log.sink()) => r,
        _ = ctx.cancel().cancelled() => return Err(Error::Canceled),
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) => {
            if docker.image_exists(image).await.unwrap_or(false) {
                ctx.log.system(format!("==> Could not pull {image} ({}); using the local copy", error_message(&e)));
                return Ok(());
            }
            Err(Error::Build(match e {
                Error::NotFound(_) => {
                    format!("image {image} not found: check the name and tag (private registries are not supported)")
                }
                other => format!("pulling image {image}: {}", error_message(&other)),
            }))
        }
    }
}

/// Store the image and commit on the deploy, and remember how it was built.
async fn record_build(ctx: &Ctx, deploy: &Deploy, built: &Built) -> ferry_core::Result<()> {
    let inner = &ctx.inner;
    let mut d = inner.store.require_deploy(&deploy.id).await?;
    d.image = Some(built.image.clone());
    // Read the source back: a restart may have been re-pointed.
    if let DeploySource::Reuse { from_deploy: Some(from), .. } = &d.source.clone()
        && let Some(src) = inner.store.get_deploy(from).await?
    {
        d.commit_sha = src.commit_sha;
        d.commit_message = src.commit_message;
    }
    inner.store.update_deploy(&d).await?;
    if let Err(e) = instances::save_build_info(&inner.store, &deploy.id, built.info).await {
        warn!(deploy = %deploy.id, "cannot record build info: {e}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// references to other services

/// Services referenced by port (`${{service.X.port}}`, `hostport`,
/// `internalUrl`) have no known port until their first deploy goes live.
/// When one of them is still deploying (e.g. a blueprint that deploys every
/// service at once), wait for it instead of failing. Bounded; circular waits
/// fail right away. Unresolvable references for any other reason are left to
/// the resolution itself (which fails the deploy with the reason).
async fn wait_for_references(ctx: &Ctx) -> Result<(), Failure> {
    let inner = &ctx.inner;
    let limit =
        Duration::from_secs(inner.config.health_check_timeout_secs.max(1).saturating_mul(2)).max(REFERENCE_WAIT_MIN);
    let deadline = Instant::now() + limit;
    let mut announced: HashSet<String> = HashSet::new();
    let result = loop {
        let svc = current_service(ctx, Stage::Build).await?;
        let pending = match pending_references(inner, &svc).await {
            Ok(p) => p,
            Err(e) => break Err(Failure::build(e)),
        };
        if pending.is_empty() {
            break Ok(());
        }
        let ids: HashSet<String> = pending.iter().map(|s| s.id.clone()).collect();
        let circular = inner.with_rt(|rt| {
            rt.reference_waits.insert(svc.id.clone(), ids.clone());
            waits_on(&rt.reference_waits, &ids, &svc.id)
        });
        if circular {
            let names: Vec<&str> = pending.iter().map(|s| s.name.as_str()).collect();
            break Err(Failure::deploy(Error::invalid(format!(
                "cannot resolve environment variables: '{}' and {} reference each other's port and \
                 neither has deployed yet: deploy one of them without the reference first",
                svc.name,
                names.join(", ")
            ))));
        }
        for s in &pending {
            if announced.insert(s.id.clone()) {
                ctx.log.system(format!(
                    "==> Waiting for service '{}' to finish deploying: its port is referenced in the environment",
                    s.name
                ));
            }
        }
        if Instant::now() >= deadline {
            // Let the resolution report which reference is unresolvable.
            break Ok(());
        }
        tokio::select! {
            _ = tokio::time::sleep(REFERENCE_POLL) => {}
            _ = ctx.cancel().cancelled() => break Err(Failure::build(Error::Canceled)),
        }
    };
    inner.with_rt(|rt| rt.reference_waits.remove(&ctx.service_id));
    result
}

/// Referenced services whose port is unknown and that have a deploy in
/// progress.
async fn pending_references(inner: &Inner, svc: &Service) -> ferry_core::Result<Vec<Service>> {
    let user = inner.store.effective_env(&svc.id).await?;
    let names = spec::port_referenced_services(&user);
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let (_, refs) = inner.store.reference_targets(&inner.config).await?;
    let services = inner.store.list_services().await?;
    let active = inner.store.active_deploys().await?;
    let mut pending = Vec::new();
    for name in names {
        if name == svc.name || refs.iter().any(|r| r.name == name && r.port.is_some()) {
            continue;
        }
        if let Some(target) = services.iter().find(|s| s.name == name)
            && active.iter().any(|d| d.service_id == target.id)
        {
            pending.push(target.clone());
        }
    }
    Ok(pending)
}

/// Whether any of `from` (transitively) waits for `target`.
fn waits_on(waits: &HashMap<String, HashSet<String>>, from: &HashSet<String>, target: &str) -> bool {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = from.iter().map(String::as_str).collect();
    while let Some(id) = stack.pop() {
        if id == target {
            return true;
        }
        if seen.insert(id)
            && let Some(next) = waits.get(id)
        {
            stack.extend(next.iter().map(String::as_str));
        }
    }
    false
}

// ---------------------------------------------------------------------------
// deploy stage

async fn deploy_stage(ctx: &Ctx, built: Built) -> Result<(), Failure> {
    let inner = &ctx.inner;
    // Exclusive access to the service's containers until the deploy is done.
    let _lock = tokio::select! {
        g = inner.service_locks.lock(&ctx.service_id) => g,
        _ = ctx.cancel().cancelled() => return Err(Failure::deploy(Error::Canceled)),
    };
    let _deploying = SetGuard::insert(inner, &ctx.service_id, deploying_set);
    inner.store.set_deploy_status(&ctx.deploy_id, DeployStatus::Deploying, None).await.map_err(Failure::deploy)?;

    let mut started: Vec<(ContainerInfo, Instant)> = Vec::new();
    let result = start_and_swap(ctx, &built, &mut started).await;
    if result.is_err() && !ctx.active.is_swapped() {
        remove_new_instances(ctx, &started).await;
    }
    result
}

async fn remove_new_instances(ctx: &Ctx, started: &[(ContainerInfo, Instant)]) {
    let containers: Vec<ContainerInfo> = started.iter().map(|(c, _)| c.clone()).collect();
    if !containers.is_empty() {
        ctx.log.system(format!("==> Removing {} new instance(s)", containers.len()));
    }
    if let Err(e) = instances::retire(&ctx.inner, &containers, 0).await {
        warn!(deploy = %ctx.deploy_id, "could not remove every new instance: {e}");
    }
    // Anything else labelled with this deploy (e.g. a half-created container).
    crate::deploy::remove_deploy_containers(&ctx.inner, &ctx.deploy_id).await;
}

/// The hint for when the instance runs but nothing accepts connections on
/// its port: the port (`$PORT`) and the address the app listens on (an app
/// bound to 127.0.0.1 inside its container is unreachable from outside it).
fn port_hint(port: u16, user_env: &[EnvVar]) -> String {
    match user_env.iter().rev().find(|v| v.key == "PORT") {
        Some(v) if v.value.trim().parse::<u16>().ok() != Some(port) => format!(
            "the app gets PORT={} from its environment but Ferry expects it to listen on port {port}; it must also \
             listen on 0.0.0.0 (an app listening on 127.0.0.1 inside its container can't be reached)",
            v.value.trim()
        ),
        _ => format!(
            "does the app listen on $PORT ({port}) on 0.0.0.0? An app listening on 127.0.0.1 (localhost) inside its \
             container can't be reached"
        ),
    }
}

async fn start_and_swap(ctx: &Ctx, built: &Built, started: &mut Vec<(ContainerInfo, Instant)>) -> Result<(), Failure> {
    let inner = &ctx.inner;
    let config = &inner.config;
    let svc = current_service(ctx, Stage::Deploy).await?;
    let desired = svc.desired_instances().max(1) as usize;

    // Environment (references resolved now), port, command, disk: the spec
    // every instance of this deploy runs with, for as long as it is live.
    let user_env = spec::resolve_user_env(inner, &svc).await.map_err(Failure::deploy)?;
    let port = if svc.listens() {
        let exposed = inner.docker.image_exposed_ports(&built.image).await.map_err(Failure::deploy)?;
        let hint = built.info.port_hint;
        let port = env::choose_port(svc.port, &user_env, hint, &exposed, config.default_port);
        let reason = instances::port_reason(svc.port, &user_env, hint, &exposed);
        ctx.log.system(format!("==> Using port {port} ({reason})"));
        if let Some(warning) = instances::port_env_mismatch(port, &user_env) {
            ctx.log.system(format!("==> Warning: {warning}"));
        }
        Some(port)
    } else {
        None
    };
    let hint = port.map(|p| port_hint(p, &user_env));
    let mut deploy = inner.store.require_deploy(&ctx.deploy_id).await.map_err(Failure::deploy)?;
    deploy.port = port;
    inner.store.update_deploy(&deploy).await.map_err(Failure::deploy)?;
    let spec = LaunchSpec::new(&svc, &deploy, built.image.clone(), port, built.info.runtime, user_env, config);
    let plan = spec::plan(inner, &svc, &spec).await.map_err(Failure::deploy)?;
    if let Some(v) = &plan.volume {
        ctx.log.system(format!("==> Mounting disk at {}", v.target));
    }
    log_resources(ctx, &svc, &plan.resources, "per instance");

    let mut old: Vec<ContainerInfo> = instances::service_containers(inner, &svc.id, true)
        .await
        .map_err(Failure::deploy)?
        .into_iter()
        .filter(|c| c.labels.get(ferry_core::naming::LABEL_DEPLOY) != Some(&ctx.deploy_id))
        .collect();

    // Services with a disk: recreate (the volume can't be shared safely).
    if svc.disk_mount_path.is_some() && !old.is_empty() {
        ctx.check_cancel(Stage::Deploy)?;
        ctx.log.system(format!(
            "==> Stopping {} old instance(s) first (services with a disk are redeployed in place)",
            old.len()
        ));
        if svc.is_public_http() {
            inner.routes.set_service_routes(&svc.id, &config.service_hosts(&svc), Vec::new());
        }
        instances::retire(inner, &old, STOP_GRACE_SECS).await.map_err(Failure::deploy)?;
        old.clear();
    }

    ctx.log.system(format!("==> Starting {desired} instance(s)"));
    for _ in 0..desired {
        ctx.check_cancel(Stage::Deploy)?;
        let container_spec = instances::service_spec(&inner.naming, &svc, &ctx.deploy_id, &plan);
        let info = inner.docker.run_container(&container_spec).await.map_err(|e| {
            Failure::deploy(Error::Docker(format!("starting an instance failed: {}", error_message(&e))))
        })?;
        let at = match (port, info.host_port) {
            (Some(p), Some(h)) => format!(" (port {p}, published on 127.0.0.1:{h})"),
            (Some(p), None) => format!(" (port {p})"),
            _ => String::new(),
        };
        ctx.log.system(format!("==> Started instance {}{at}", instance_id(&info.name)));
        started.push((info, Instant::now()));
    }

    // Health checks, with the new instances' output in the deploy log.
    let timeout_secs = config.health_check_timeout_secs.max(1);
    let check = HealthCheck {
        probe: Probe::for_service(&svc, config.default_host(&svc.name)),
        deadline: Instant::now() + Duration::from_secs(timeout_secs),
        timeout_secs,
        container_port: port,
        port_hint: hint,
        worker: svc.service_type == ServiceType::BackgroundWorker,
        progress: ctx.log.sink().clone(),
    };
    ctx.log.system(format!(
        "==> Waiting for {} instance(s) to become healthy: {} (timeout {timeout_secs}s)",
        started.len(),
        check.probe.describe()
    ));
    let containers: Vec<ContainerInfo> = started.iter().map(|(c, _)| c.clone()).collect();
    let mut capture = OutputCapture::start(&inner.docker, &containers, ctx.log.sink());
    let checks = try_join_all(started.iter().map(|(c, at)| wait_healthy(inner, &check, c, *at)));
    let outcome = tokio::select! {
        r = checks => Some(r),
        _ = ctx.cancel().cancelled() => None,
    };
    capture.stop().await;
    let verified_ports = match outcome {
        None => return Err(Failure::deploy(Error::Canceled)),
        Some(Err(failure)) => {
            if failure.crashed {
                capture.dump_tail(&inner.docker, &failure.container, ctx.log.sink(), CRASH_TAIL_LINES).await;
            }
            return Err(Failure::deploy(Error::Invalid(failure.message)));
        }
        Some(Ok(ports)) => ports,
    };
    ctx.log.system("==> Health check passed");

    // The point of no return: after this the deploy can no longer be
    // canceled, and it goes live even if the server stops right after.
    if !ctx.active.commit_swap() {
        return Err(Failure::deploy(Error::Canceled));
    }
    let svc = inner.store.get_service(&svc.id).await.ok().flatten().unwrap_or(svc);
    if svc.is_public_http() {
        // The ports the health checks verified (fresher than the start info).
        let upstreams: Vec<SocketAddr> = verified_ports
            .iter()
            .zip(started.iter())
            .filter_map(|(verified, (c, _))| verified.or(c.host_port))
            .map(|p| SocketAddr::from(([127, 0, 0, 1], p)))
            .collect();
        inner.routes.set_service_routes(&svc.id, &config.service_hosts(&svc), upstreams);
        ctx.log.system("==> Routing traffic to the new instance(s)");
    }
    // Live right away (before the old instances are drained): if the server
    // stops or crashes while draining, it comes back with this deploy.
    mark_live(ctx, &spec).await.map_err(Failure::deploy)?;

    if !old.is_empty() {
        if svc.is_public_http() {
            tokio::select! {
                _ = tokio::time::sleep(DRAIN_DELAY) => {}
                _ = inner.shutdown.cancelled() => {}
            }
        }
        ctx.log.system(format!("==> Stopping {} old instance(s)", old.len()));
        if let Err(e) = instances::retire(inner, &old, STOP_GRACE_SECS).await {
            ctx.log.system(format!(
                "==> Warning: not every old instance could be removed ({}); it will be retried",
                error_message(&e)
            ));
        }
    }
    announce_live(ctx, port).await;
    Ok(())
}

/// The effective limits in the deploy log (`==> Limits: 512 MiB memory,
/// 1 CPU per instance`), with a warning when they were capped (also in the
/// server log) or won't protect the host.
fn log_resources(ctx: &Ctx, svc: &Service, resources: &Resources, unit: &str) {
    ctx.log.system(format!("==> Limits: {} {unit}", resources.summary()));
    for warning in resources.cpu_cap_warning().into_iter().chain(resources.cpu_unsupported_warning()) {
        warn!(service = %svc.name, deploy = %ctx.deploy_id, "{warning}");
    }
    for warning in resources.warnings() {
        ctx.log.system(format!("==> Warning: {warning}"));
    }
}

/// Mark the deploy live (storing its launch spec first), the previous live
/// deploy deactivated.
async fn mark_live(ctx: &Ctx, spec: &LaunchSpec) -> ferry_core::Result<()> {
    let inner = &ctx.inner;
    spec::save(&inner.store, &ctx.deploy_id, spec).await?;
    let svc = inner.store.require_service(&ctx.service_id).await?;
    let previous = svc.live_deploy_id.clone();
    inner.store.set_deploy_status(&ctx.deploy_id, DeployStatus::Live, None).await?;
    inner.store.set_live_deploy(&svc.id, Some(&ctx.deploy_id)).await?;
    if let Some(prev) = previous.filter(|p| *p != ctx.deploy_id)
        && let Some(d) = inner.store.get_deploy(&prev).await?
        && d.status == DeployStatus::Live
    {
        inner.store.set_deploy_status(&prev, DeployStatus::Deactivated, None).await?;
    }
    if ctx.resume && svc.suspended {
        inner.store.set_suspended(&svc.id, false).await?;
        ctx.log.system(format!("==> Resumed '{}'", svc.name));
        info!(service = %svc.name, deploy = %ctx.deploy_id, "service resumed by a deploy");
    }
    info!(service = %svc.name, deploy = %ctx.deploy_id, "deploy live");
    Ok(())
}

/// Say that the deploy is live, and where.
async fn announce_live(ctx: &Ctx, port: Option<u16>) {
    let inner = &ctx.inner;
    let svc = match inner.store.get_service(&ctx.service_id).await {
        Ok(Some(s)) => s,
        _ => {
            ctx.log.system("==> Your service is live 🎉");
            return;
        }
    };
    if svc.service_type == ServiceType::CronJob {
        let schedule = svc.schedule.as_deref().unwrap_or("?");
        ctx.log.system(format!("==> Your cron job is live 🎉 (schedule: {schedule}, UTC)"));
        return;
    }
    ctx.log.system("==> Your service is live 🎉");
    if svc.is_public_http() {
        for host in inner.config.service_hosts(&svc) {
            ctx.log.system(format!("==> Available at {}", inner.config.url_for_host(&host)));
        }
    } else if let Some(p) = port {
        ctx.log.system(format!("==> Reachable on the private network at {}:{p}", svc.name));
    }
}

/// The failure line of a deploy log: named after the stage that failed,
/// like the deploy's status (`build_failed` / `deploy_failed`).
pub(crate) fn failure_line(status: DeployStatus, message: &str) -> String {
    match status {
        DeployStatus::BuildFailed => format!("==> Build failed: {message}"),
        _ => format!("==> Deploy failed: {message}"),
    }
}

async fn record_failure(ctx: &Ctx, failure: Failure) {
    let inner = &ctx.inner;
    let failed_status = match failure.stage {
        Stage::Build => DeployStatus::BuildFailed,
        Stage::Deploy => DeployStatus::DeployFailed,
    };
    let (status, message, line) = match &failure.error {
        Error::Canceled => match ctx.active.cancel_reason() {
            Some(reason) => (DeployStatus::Canceled, reason.clone(), format!("==> Deploy canceled: {reason}")),
            None if inner.shutdown.is_cancelled() => {
                let m = "interrupted by server shutdown".to_string();
                (failed_status, m.clone(), failure_line(failed_status, &m))
            }
            None => (DeployStatus::Canceled, "canceled".to_string(), "==> Deploy canceled".to_string()),
        },
        other => {
            let m = error_message(other);
            (failed_status, m.clone(), failure_line(failed_status, &m))
        }
    };
    if !failure.logged {
        ctx.log.system(line);
    }
    info!(deploy = %ctx.deploy_id, status = %status, "deploy ended: {message}");
    if let Err(e) = inner.store.set_deploy_status(&ctx.deploy_id, status, Some(&message)).await {
        warn!(deploy = %ctx.deploy_id, "cannot record the deploy's failure: {e}");
    }
}

/// The deploy task panicked: fail it and remove whatever it started.
pub(crate) async fn handle_crash(inner: &Arc<Inner>, deploy: &Deploy, log: &LogHandle, message: &str) {
    match inner.store.get_deploy(&deploy.id).await {
        Ok(Some(d)) if d.status.is_active() => {
            let status = if d.status == DeployStatus::Deploying {
                DeployStatus::DeployFailed
            } else {
                DeployStatus::BuildFailed
            };
            log.system(failure_line(status, message));
            if let Err(e) = inner.store.set_deploy_status(&d.id, status, Some(message)).await {
                warn!(deploy = %d.id, "cannot record the deploy's failure: {e}");
            }
            crate::deploy::remove_deploy_containers(inner, &d.id).await;
        }
        // Already live (it failed while draining the old instances): the
        // reconciler removes what is left of them.
        Ok(_) => log.system(format!("==> Warning: {message}")),
        Err(e) => warn!(deploy = %deploy.id, "cannot read the crashed deploy: {e}"),
    }
}

// ---------------------------------------------------------------------------
// output capture during health checks

/// What was already copied from one container (to avoid duplicates when its
/// last lines are dumped after a crash).
#[derive(Debug, Default, Clone)]
struct Seen {
    last_ts: Option<DateTime<Utc>>,
    at_last: Vec<String>,
}

impl Seen {
    fn record(&mut self, line: &LogLine) {
        match self.last_ts {
            Some(t) if line.ts == t => self.at_last.push(line.line.clone()),
            Some(t) if line.ts < t => {}
            _ => {
                self.last_ts = Some(line.ts);
                self.at_last = vec![line.line.clone()];
            }
        }
    }

    fn covers(&self, line: &LogLine) -> bool {
        self.last_ts.is_some_and(|t| line.ts < t || (line.ts == t && self.at_last.contains(&line.line)))
    }
}

/// Streams the output of new instances into the deploy log while they are
/// health-checked.
struct OutputCapture {
    stop: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    seen: Arc<StdMutex<HashMap<String, Seen>>>,
}

impl OutputCapture {
    fn start(docker: &Docker, containers: &[ContainerInfo], sink: &LogSink) -> Self {
        let stop = CancellationToken::new();
        let seen: Arc<StdMutex<HashMap<String, Seen>>> = Arc::default();
        let tasks = containers
            .iter()
            .map(|c| {
                let mut lines = docker.logs(&c.id, true, None);
                let (stop, seen, sink) = (stop.clone(), seen.clone(), sink.clone());
                let (id, instance) = (c.id.clone(), instance_id(&c.name));
                tokio::spawn(async move {
                    loop {
                        let line = tokio::select! {
                            _ = stop.cancelled() => break,
                            l = lines.next() => l,
                        };
                        let Some(line) = line else { break };
                        lock(&seen).entry(id.clone()).or_default().record(&line);
                        sink.send(line.with_instance(instance.clone()));
                    }
                })
            })
            .collect();
        OutputCapture { stop, tasks, seen }
    }

    async fn stop(&mut self) {
        self.stop.cancel();
        for task in self.tasks.drain(..) {
            if tokio::time::timeout(Duration::from_secs(2), task).await.is_err() {
                debug!("output capture task did not stop in time");
            }
        }
    }

    /// Copy the last `n` lines of a crashed instance that are not already in
    /// the deploy log.
    async fn dump_tail(&self, docker: &Docker, container: &ContainerInfo, sink: &LogSink, n: usize) {
        let lines: Vec<LogLine> = docker.logs(&container.id, false, Some(n)).collect().await;
        let seen = lock(&self.seen).get(&container.id).cloned().unwrap_or_default();
        let unseen: Vec<LogLine> = lines.into_iter().filter(|l| !seen.covers(l)).collect();
        if unseen.is_empty() {
            return;
        }
        let instance = instance_id(&container.name);
        sink.system(format!("==> Last output of instance {instance}:"));
        for line in unseen {
            sink.send(line.with_instance(instance.clone()));
        }
    }
}

// ---------------------------------------------------------------------------
// cleanup

/// After a deploy: delete old images of the service (keeping the newest
/// `keep_images`, the live one and those of active deploys) and uploaded
/// archives that can no longer be deployed.
pub(crate) async fn cleanup_after_deploy(inner: &Arc<Inner>, service_id: &str) {
    let svc = match inner.store.get_service(service_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return,
        Err(e) => {
            warn!(service = service_id, "cleanup skipped: {e}");
            return;
        }
    };
    let (deploys, active) = match (inner.store.deploys_with_images(&svc.id).await, inner.store.active_deploys().await) {
        (Ok(d), Ok(a)) => (d, a),
        (Err(e), _) | (_, Err(e)) => {
            warn!(service = %svc.name, "cleanup skipped: {e}");
            return;
        }
    };
    let mut protected: HashSet<String> = HashSet::new();
    if let Some(live) = &svc.live_deploy_id
        && let Some(image) = deploys.iter().find(|d| &d.id == live).and_then(|d| d.image.clone())
    {
        protected.insert(image);
    }
    for d in active.iter().filter(|d| d.service_id == svc.id) {
        if let Some(image) = &d.image {
            protected.insert(image.clone());
        }
        if let DeploySource::Reuse { image, .. } = &d.source {
            protected.insert(image.clone());
        }
    }
    let repo = inner.naming.image_repo(&svc.name);
    for image in images::images_to_remove(&deploys, &repo, inner.config.keep_images, &protected) {
        match inner.docker.remove_image(&image).await {
            Ok(()) => {
                info!(service = %svc.name, image = %image, "removed old image");
                let ids: Vec<String> = deploys
                    .iter()
                    .filter(|d| d.image.as_deref() == Some(image.as_str()))
                    .map(|d| d.id.clone())
                    .collect();
                instances::forget_deploys(&inner.store, &ids).await;
            }
            Err(Error::Conflict(m)) => debug!(image = %image, "image still in use, kept: {m}"),
            Err(e) => warn!(image = %image, "cannot remove old image: {e}"),
        }
    }
    // Only the live deploy's launch spec is ever used again: drop those of
    // the recent deploys that are no longer (or never were) live. Older ones
    // were dropped by earlier cleanups.
    let stale_specs: Vec<String> = deploys
        .iter()
        .take(SPEC_GC_WINDOW)
        .filter(|d| Some(&d.id) != svc.live_deploy_id.as_ref() && !d.status.is_active())
        .map(|d| d.id.clone())
        .collect();
    spec::forget(&inner.store, &stale_specs).await;
    cleanup_uploads(inner, &svc).await;
}

/// Uploaded archives other than the newest one (what a plain `deploy` of an
/// upload-only service uses) and those of active deploys are deleted.
async fn cleanup_uploads(inner: &Inner, svc: &Service) {
    let deploys = match inner.store.list_deploys(&svc.id, 10_000).await {
        Ok(d) => d,
        Err(e) => {
            debug!(service = %svc.name, "upload cleanup skipped: {e}");
            return;
        }
    };
    let mut keep: HashSet<&str> = HashSet::new();
    let mut newest = true;
    for d in &deploys {
        if let DeploySource::Archive { path } = &d.source {
            if newest || d.status.is_active() {
                keep.insert(path.as_str());
            }
            newest = false;
        }
    }
    let mut removed: HashSet<&str> = HashSet::new();
    for d in &deploys {
        if let DeploySource::Archive { path } = &d.source
            && !keep.contains(path.as_str())
            && removed.insert(path.as_str())
        {
            remove_upload(inner, Path::new(path)).await;
        }
    }
}

/// Delete an uploaded archive, but only if it really is in the uploads dir.
pub(crate) async fn remove_upload(inner: &Inner, path: &Path) {
    let (Ok(file), Ok(dir)) =
        (tokio::fs::canonicalize(path).await, tokio::fs::canonicalize(inner.config.uploads_dir()).await)
    else {
        return;
    };
    if file.parent() != Some(dir.as_path()) {
        debug!(path = %path.display(), "not deleting a file outside the uploads directory");
        return;
    }
    match tokio::fs::remove_file(&file).await {
        Ok(()) => debug!(path = %file.display(), "removed old upload"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(path = %file.display(), "cannot remove old upload: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: i64, text: &str) -> LogLine {
        let mut l = LogLine::system(text);
        l.ts = DateTime::from_timestamp(ts, 0).unwrap();
        l
    }

    #[test]
    fn refused_clones_get_a_hint() {
        use ferry_core::GitProvider;
        let url = "https://github.com/acme/app.git";
        let account = GitConnection::new(GitProvider::Github, "https://github.com", "octocat", "ghp_token");
        let build = |m: &str| Error::Build(format!("git fetch from {url} failed: {m}"));
        for refused in [
            "Authentication failed for 'https://github.com/acme/app.git/'",
            "could not read Username for 'https://github.com': terminal prompts disabled",
            "unable to get password from user",
            "repository 'https://github.com/acme/app.git/' not found",
            "unable to access 'https://github.com/acme/app.git/': The requested URL returned error: 403",
            "HTTP Basic: Access denied",
        ] {
            let with = git_access_hint(&build(refused), Some(&account), url).unwrap_or_default();
            assert!(with.starts_with("==> Hint: the token of the GitHub account 'octocat' may have expired"), "{with}");
            assert!(!with.contains("ghp_"), "{with}");
            let without = git_access_hint(&build(refused), None, url).unwrap_or_default();
            assert!(without.starts_with("==> Hint: if the repository is private, connect its"), "{without}");
            // ssh and local repositories don't use connections.
            assert_eq!(git_access_hint(&build(refused), None, "git@github.com:acme/app.git"), None);
            assert_eq!(git_access_hint(&build(refused), None, "/srv/repos/app"), None);
        }
        // Other failures aren't about access.
        for other in
            ["Could not resolve host: github.com", "branch 'main' not found in https://github.com/acme/app.git"]
        {
            assert_eq!(git_access_hint(&build(other), Some(&account), url), None, "{other}");
        }
        assert_eq!(git_access_hint(&Error::Canceled, Some(&account), url), None);
    }

    #[test]
    fn seen_tracking_dedups_crash_tails() {
        let mut seen = Seen::default();
        assert!(!seen.covers(&line(1, "a")));
        seen.record(&line(1, "a"));
        seen.record(&line(2, "b"));
        seen.record(&line(2, "c"));
        seen.record(&line(1, "late stderr")); // out of order: ignored for tracking
        assert!(seen.covers(&line(1, "a")));
        assert!(seen.covers(&line(2, "b")));
        assert!(seen.covers(&line(2, "c")));
        assert!(!seen.covers(&line(2, "d")), "same timestamp, different text");
        assert!(!seen.covers(&line(3, "e")));
    }

    #[test]
    fn failure_lines_name_the_failed_stage() {
        assert_eq!(failure_line(DeployStatus::BuildFailed, "no such image"), "==> Build failed: no such image");
        assert_eq!(failure_line(DeployStatus::DeployFailed, "crashed"), "==> Deploy failed: crashed");
    }

    #[test]
    fn port_hints_point_at_the_port_variable_and_the_bind_address() {
        let h = port_hint(8000, &[]);
        assert!(h.starts_with("does the app listen on $PORT (8000) on 0.0.0.0?"), "{h}");
        assert!(h.contains("An app listening on 127.0.0.1 (localhost) inside its container can't be reached"), "{h}");
        assert_eq!(port_hint(8000, &[EnvVar::new("PORT", "8000")]), h);
        let h = port_hint(8000, &[EnvVar::new("PORT", "9000")]);
        assert!(h.contains("PORT=9000") && h.contains("8000"), "{h}");
        assert!(h.contains("0.0.0.0") && h.contains("127.0.0.1"), "{h}");
    }

    #[test]
    fn circular_reference_waits_are_detected() {
        let mut waits: HashMap<String, HashSet<String>> = HashMap::new();
        let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<HashSet<String>>();
        waits.insert("a".into(), set(&["b"]));
        waits.insert("b".into(), set(&["c"]));
        assert!(!waits_on(&waits, &set(&["b"]), "a"));
        waits.insert("c".into(), set(&["a"]));
        assert!(waits_on(&waits, &set(&["b"]), "a"), "a → b → c → a");
        assert!(!waits_on(&waits, &set(&["x"]), "a"));
    }

    #[test]
    fn start_lines() {
        let d = Deploy::new("srv-1", DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        assert!(start_line(&d).contains("(trigger: manual)"));
        let r = Deploy::new(
            "srv-1",
            DeployTrigger::Rollback,
            DeploySource::Reuse { image: "x".into(), from_deploy: Some("dep-old".into()) },
        );
        assert!(start_line(&r).starts_with("==> Rolling back to deploy dep-old"));
    }
}
