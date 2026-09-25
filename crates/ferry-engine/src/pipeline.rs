//! One deploy, start to finish: build (or pull / reuse) → start instances →
//! health check → route swap → drain the old instances → live. On failure
//! the new instances are removed and the old ones keep serving.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ferry_build::{BuildRequest, BuildSource};
use ferry_core::{
    Deploy, DeploySource, DeployStatus, DeployTrigger, Error, LogLine, LogSink, Runtime, Service, ServiceType, env,
};
use ferry_docker::{ContainerInfo, Docker};
use futures::StreamExt;
use futures::future::try_join_all;
use tokio::sync::OwnedSemaphorePermit;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::health::{Probe, wait_healthy};
use crate::images;
use crate::instances::{self, BuildInfo, Plan, STOP_GRACE_SECS};
use crate::logs::LogHandle;
use crate::state::{ActiveDeploy, DeployOptions, Inner, SetGuard, deploying_set};
use crate::util::{error_message, instance_id, lock};

/// Pause between routing traffic to the new instances and stopping the old
/// ones, so requests already in flight on the old instances can finish.
const DRAIN_DELAY: Duration = Duration::from_secs(2);
/// Log lines of a crashed instance copied into the deploy log.
const CRASH_TAIL_LINES: usize = 50;

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
    let ctx = Ctx { inner, deploy_id: deploy.id.clone(), service_id: deploy.service_id.clone(), active, log };
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
    info!(service = %svc.name, deploy = %deploy.id, "deploy started");

    let built = build_stage(ctx, &svc, &deploy, opts).await?;
    drop(permit);
    record_build(ctx, &deploy, &built).await.map_err(Failure::build)?;
    ctx.check_cancel(Stage::Build)?;

    if svc.service_type == ServiceType::CronJob {
        return go_live(ctx, None).await.map_err(Failure::deploy);
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

// ---------------------------------------------------------------------------
// build stage

async fn build_stage(ctx: &Ctx, svc: &Service, deploy: &Deploy, opts: DeployOptions) -> Result<Built, Failure> {
    let inner = &ctx.inner;
    match &deploy.source {
        DeploySource::Git { repo_url, branch, commit } => {
            let source =
                BuildSource::Git { repo_url: repo_url.clone(), branch: branch.clone(), commit: commit.clone() };
            build_image(ctx, svc, deploy, source, opts).await
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
            Ok(Built { image, info: BuildInfo { runtime: Some(Runtime::Image), port_hint: None } })
        }
        DeploySource::Reuse { image, from_deploy } => {
            let (image, from_deploy) =
                current_reuse_target(ctx, svc, deploy, image, from_deploy.as_deref()).await.map_err(Failure::build)?;
            let (image, from_deploy) = (&image, &from_deploy);
            let exists = inner.docker.image_exists(image).await.map_err(Failure::build)?;
            if !exists {
                return Err(Failure::build(Error::Build(format!(
                    "image {image} no longer exists (only the newest {} built images of a service are kept): \
                     deploy again from source",
                    inner.config.keep_images
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

/// A restart queued while another deploy was running must reuse what is
/// live *now* (else it would roll that deploy back): re-point its source at
/// the current live deploy. Rollbacks keep their explicit target.
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
    let Some(live_id) = svc.live_deploy_id.as_deref().filter(|id| Some(*id) != from_deploy) else {
        return Ok(unchanged);
    };
    let Some(live_image) = inner.store.get_deploy(live_id).await?.and_then(|d| d.image) else {
        return Ok(unchanged);
    };
    let mut d = inner.store.require_deploy(&deploy.id).await?;
    d.source = DeploySource::Reuse { image: live_image.clone(), from_deploy: Some(live_id.to_string()) };
    inner.store.update_deploy(&d).await?;
    ctx.log.system(format!("==> The live deploy changed since this restart was queued: restarting {live_id}"));
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
    match inner.builder.build(&req, ctx.log.sink(), ctx.cancel()).await {
        Ok(out) => {
            // Record the commit right away (visible while deploying).
            if out.commit_sha.is_some() || out.commit_message.is_some() {
                match inner.store.require_deploy(&deploy.id).await {
                    Ok(mut d) => {
                        d.commit_sha = out.commit_sha.clone();
                        d.commit_message = out.commit_message.clone();
                        if let Err(e) = inner.store.update_deploy(&d).await {
                            warn!(deploy = %deploy.id, "cannot record the commit: {e}");
                        }
                    }
                    Err(e) => warn!(deploy = %deploy.id, "cannot record the commit: {e}"),
                }
            }
            Ok(Built { image: out.image, info: BuildInfo { runtime: Some(out.runtime), port_hint: out.port_hint } })
        }
        Err(Error::Canceled) => Err(Failure::build(Error::Canceled)),
        // The builder already wrote "==> Build failed: <reason>".
        Err(e) => Err(Failure { stage: Stage::Build, error: e, logged: true }),
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
    let mut swapped = false;
    let result = start_and_swap(ctx, &built, &mut started, &mut swapped).await;
    if result.is_err() && !swapped {
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

async fn start_and_swap(
    ctx: &Ctx,
    built: &Built,
    started: &mut Vec<(ContainerInfo, Instant)>,
    swapped: &mut bool,
) -> Result<(), Failure> {
    let inner = &ctx.inner;
    let config = &inner.config;
    let svc = match inner.store.get_service(&ctx.service_id).await.map_err(Failure::deploy)? {
        Some(s) => s,
        None => return Err(ctx.canceled(Stage::Deploy, "service deleted")),
    };
    if svc.suspended {
        return Err(ctx.canceled(Stage::Deploy, "service suspended"));
    }
    let desired = svc.desired_instances().max(1) as usize;

    // Port, env, command, disk.
    let user_env = instances::user_env(inner, &svc).await.map_err(Failure::deploy)?;
    let port = if svc.listens() {
        let exposed = inner.docker.image_exposed_ports(&built.image).await.map_err(Failure::deploy)?;
        let hint = built.info.port_hint;
        let port = env::choose_port(svc.port, &user_env, hint, &exposed, config.default_port);
        let reason = instances::port_reason(svc.port, &user_env, hint, &exposed);
        ctx.log.system(format!("==> Using port {port} ({reason})"));
        Some(port)
    } else {
        None
    };
    let mut deploy = inner.store.require_deploy(&ctx.deploy_id).await.map_err(Failure::deploy)?;
    deploy.port = port;
    inner.store.update_deploy(&deploy).await.map_err(Failure::deploy)?;
    let env = instances::container_env(inner, &svc, &deploy, port).await.map_err(Failure::deploy)?;
    let volume = instances::disk_volume(inner, &svc).await.map_err(Failure::deploy)?;
    if let Some(v) = &volume {
        ctx.log.system(format!("==> Mounting disk at {}", v.target));
    }
    let plan =
        Plan { image: built.image.clone(), port, env, cmd: instances::start_cmd(&svc, built.info.runtime), volume };

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
        let spec = instances::service_spec(&inner.naming, &svc, &ctx.deploy_id, &plan);
        let info = inner.docker.run_container(&spec).await.map_err(|e| {
            Failure::deploy(Error::Docker(format!("starting an instance failed: {}", error_message(&e))))
        })?;
        let at = match info.host_port {
            Some(p) => format!(" (127.0.0.1:{p})"),
            None => String::new(),
        };
        ctx.log.system(format!("==> Started instance {}{at}", instance_id(&info.name)));
        started.push((info, Instant::now()));
    }

    // Health checks, with the new instances' output in the deploy log.
    let probe = Probe::for_service(&svc, config.default_host(&svc.name));
    let timeout_secs = config.health_check_timeout_secs.max(1);
    ctx.log.system(format!(
        "==> Waiting for {} instance(s) to become healthy: {} (timeout {timeout_secs}s)",
        started.len(),
        probe.describe()
    ));
    let containers: Vec<ContainerInfo> = started.iter().map(|(c, _)| c.clone()).collect();
    let mut capture = OutputCapture::start(&inner.docker, &containers, ctx.log.sink());
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let checks =
        try_join_all(started.iter().map(|(c, at)| wait_healthy(inner, c, &probe, *at, deadline, timeout_secs)));
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
    ctx.check_cancel(Stage::Deploy)?;

    // Swap: from here on the new instances serve traffic and are kept.
    let svc = inner.store.get_service(&svc.id).await.ok().flatten().unwrap_or(svc);
    *swapped = true;
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
    if !old.is_empty() {
        if svc.is_public_http() {
            tokio::time::sleep(DRAIN_DELAY).await;
        }
        ctx.log.system(format!("==> Stopping {} old instance(s)", old.len()));
        if let Err(e) = instances::retire(inner, &old, STOP_GRACE_SECS).await {
            ctx.log.system(format!(
                "==> Warning: not every old instance could be removed ({}); it will be retried",
                error_message(&e)
            ));
        }
    }
    go_live(ctx, port).await.map_err(Failure::deploy)
}

/// Mark the deploy live, the previous live deploy deactivated, and say so.
async fn go_live(ctx: &Ctx, port: Option<u16>) -> ferry_core::Result<()> {
    let inner = &ctx.inner;
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
    info!(service = %svc.name, deploy = %ctx.deploy_id, "deploy live");
    if svc.service_type == ServiceType::CronJob {
        let schedule = svc.schedule.as_deref().unwrap_or("?");
        ctx.log.system(format!("==> Your cron job is live 🎉 (schedule: {schedule}, UTC)"));
        return Ok(());
    }
    ctx.log.system("==> Your service is live 🎉");
    if svc.is_public_http() {
        for host in inner.config.service_hosts(&svc) {
            ctx.log.system(format!("==> Available at {}", inner.config.url_for_host(&host)));
        }
    } else if let Some(p) = port {
        ctx.log.system(format!("==> Reachable on the private network at {}:{p}", svc.name));
    }
    Ok(())
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
                (failed_status, m.clone(), format!("==> Deploy failed: {m}"))
            }
            None => (DeployStatus::Canceled, "canceled".to_string(), "==> Deploy canceled".to_string()),
        },
        other => {
            let m = error_message(other);
            (failed_status, m.clone(), format!("==> Deploy failed: {m}"))
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
    log.system(format!("==> Deploy failed: {message}"));
    match inner.store.get_deploy(&deploy.id).await {
        Ok(Some(d)) if d.status.is_active() => {
            let status = if d.status == DeployStatus::Deploying {
                DeployStatus::DeployFailed
            } else {
                DeployStatus::BuildFailed
            };
            if let Err(e) = inner.store.set_deploy_status(&d.id, status, Some(message)).await {
                warn!(deploy = %d.id, "cannot record the deploy's failure: {e}");
            }
            crate::deploy::remove_deploy_containers(inner, &d.id).await;
        }
        Ok(_) => {}
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
                instances::delete_build_info(&inner.store, &ids).await;
            }
            Err(Error::Conflict(m)) => debug!(image = %image, "image still in use, kept: {m}"),
            Err(e) => warn!(image = %image, "cannot remove old image: {e}"),
        }
    }
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
