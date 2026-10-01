//! One-off jobs (`ferry run`), cron runs and the cron scheduler.
//!
//! A run is registered in `Runtime::jobs` before its row is created, so
//! suspend and delete (which set their flag first, then look there) never
//! miss one. A run ends `succeeded`, `failed` (non-zero exit, server
//! shutdown) or `canceled` (`cancel_job`, service suspended or deleted; its
//! container gets a grace period). After each run, the service's finished
//! runs beyond `config.keep_job_runs` are deleted with their logs.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ferry_core::naming::{LABEL_INSTANCE, LABEL_JOB};
use ferry_core::schedule::Schedule;
use ferry_core::{Error, JobRun, JobStatus, JobTrigger, Result, Service, ServiceType};
use ferry_docker::{ContainerSpec, RestartPolicy};
use futures::StreamExt;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::health::{LastExit, SIGKILL_EXIT_CODE};
use crate::images;
use crate::instances;
use crate::logs::{LogHandle, LogKind};
use crate::spec::{self, LaunchSpec};
use crate::state::{Inner, RunningJob};
use crate::util::{error_message, panic_message};

/// Scheduler tick interval.
pub(crate) const TICK: Duration = Duration::from_secs(15);
/// Grace period when a job is stopped (cancel, suspend, delete, shutdown).
const JOB_STOP_GRACE_SECS: u32 = 10;
/// How long a job's output may keep draining after its container exited.
const OUTPUT_DRAIN: Duration = Duration::from_secs(10);
/// How long `cancel_job`, suspend and delete wait for a job to stop.
const STOP_WAIT: Duration = Duration::from_secs(JOB_STOP_GRACE_SECS as u64 + 20);
/// How long (polls × interval) a SIGKILLed job's container is watched for
/// Docker's OOM flag, which can be recorded just after the exit. Its Docker
/// events are then asked a while longer (`health::with_late_oom`).
const OOM_FLAG_POLLS: usize = 10;
const OOM_FLAG_POLL: Duration = Duration::from_millis(100);
/// The error of a job a user canceled.
const CANCELED_BY_USER: &str = "canceled by user";
/// The error of a job stopped by the server shutting down.
const SHUTDOWN: &str = "interrupted by server shutdown";

/// `Engine::run_job`.
pub(crate) async fn run_job(
    inner: &Arc<Inner>,
    service_id: &str,
    command: Option<String>,
    trigger: JobTrigger,
) -> Result<JobRun> {
    let svc = inner.store.require_service(service_id).await?;
    if inner.is_service_deleting(&svc.id) {
        return Err(deleting(&svc));
    }
    if svc.suspended {
        return Err(suspended(&svc));
    }
    if inner.shutdown.is_cancelled() {
        return Err(Error::conflict("the server is shutting down: run the job again once it is back"));
    }
    let command = command.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if svc.service_type != ServiceType::CronJob && command.is_none() {
        return Err(Error::invalid(format!(
            "a command is required to run a job on '{}' (only cron jobs have a default command)",
            svc.name
        )));
    }
    let live_id = svc
        .live_deploy_id
        .clone()
        .ok_or_else(|| Error::conflict(format!("service '{}' has no live deploy yet: deploy it first", svc.name)))?;
    let live = inner.store.require_deploy(&live_id).await?;
    let image = live
        .image
        .clone()
        .ok_or_else(|| Error::conflict(format!("the live deploy {} of '{}' has no image", live.id, svc.name)))?;

    // Jobs run exactly like the live deploy's instances (its launch spec),
    // not with settings or env changed since it went live.
    let spec = spec::for_deploy(inner, &svc, &live).await;

    let mut job = JobRun::new(&svc.id, trigger, command);
    job.image = Some(spec.as_ref().map_or(image, |s| s.image.clone()));

    // Registered before the row exists, atomically with the deletion check.
    let (done_tx, done_rx) = watch::channel(false);
    let running = RunningJob::new(&svc.id, &job.id, inner.shutdown.child_token(), done_rx);
    let registered = inner.with_rt(|rt| {
        if rt.deleting_services.contains(&svc.id) {
            return false;
        }
        rt.jobs.insert(job.id.clone(), running.clone());
        true
    });
    if !registered {
        return Err(deleting(&svc));
    }
    let admitted = async {
        // Suspended meanwhile? (suspend stores the flag, then looks for jobs.)
        if inner.store.require_service(&svc.id).await?.suspended {
            return Err(suspended(&svc));
        }
        inner.store.create_job_run(&job).await
    }
    .await;
    if let Err(e) = admitted {
        inner.with_rt(|rt| rt.jobs.remove(&job.id));
        let _ = done_tx.send(true);
        return Err(e);
    }
    let log = inner.logs.open(LogKind::Job, &job.id);
    info!(service = %svc.name, job = %job.id, trigger = %trigger, "job started");

    let (inner2, job2) = (inner.clone(), job.clone());
    inner.spawn(async move {
        let task = {
            let (inner, job, log, running) = (inner2.clone(), job2.clone(), log.clone(), running.clone());
            inner2.spawn(async move { execute(&inner, &svc, spec, job, &log, &running).await })
        };
        if let Err(e) = task.await {
            error!(job = %job2.id, "job task failed: {e}");
            let m = panic_message(&e);
            log.system(format!("==> Job failed: {m}"));
            finish_job(&inner2, &job2.id, JobStatus::Failed, None, Some(m)).await;
            remove_job_containers(&inner2, &job2.id, 0).await;
        }
        log.finish().await;
        prune_runs(&inner2, &job2.service_id, Some(&job2.id)).await;
        inner2.with_rt(|rt| rt.jobs.remove(&job2.id));
        let _ = done_tx.send(true);
    });
    Ok(job)
}

fn deleting(svc: &Service) -> Error {
    Error::conflict(format!("service '{}' is being deleted", svc.name))
}

fn suspended(svc: &Service) -> Error {
    Error::conflict(format!("service '{}' is suspended", svc.name))
}

/// Run the job's container to completion (or until it is stopped).
async fn execute(
    inner: &Arc<Inner>,
    svc: &Service,
    spec: Result<LaunchSpec>,
    job: JobRun,
    log: &LogHandle,
    running: &RunningJob,
) {
    let image = job.image.clone().unwrap_or_default();
    let container_name = inner.naming.job_container(&svc.name, &job.id);
    let prepared = async {
        let spec = spec?;
        if !inner.docker.image_exists(&image).await? {
            // The live deploy's image: never deleted by retention.
            let reason = images::missing_reason(inner, svc, &image, true).await;
            return Err(Error::conflict(format!(
                "the image of the live deploy ({image}) no longer exists: {reason} — deploy the service again"
            )));
        }
        // The service's disk, where the live instance has it.
        let volume = instances::disk_volume(inner, &svc.id, spec.disk_mount_path.as_deref()).await?;
        Ok((spec, volume))
    }
    .await;
    let (spec, volume) = match prepared {
        Ok(p) => p,
        Err(e) => {
            let m = error_message(&e);
            log.system(format!("==> Job failed: {m}"));
            finish_job(inner, &job.id, JobStatus::Failed, None, Some(m)).await;
            return;
        }
    };
    if running.cancel.is_cancelled() {
        stopped_before_start(inner, &job.id, log, running).await;
        return;
    }
    // The command given, else the start command the live deploy runs with.
    let command = job.command.clone().or_else(|| spec.start_command.clone()).filter(|c| !c.trim().is_empty());
    if let Some(v) = &volume {
        log.system(format!("==> Mounting the service's disk at {}", v.target));
    }
    // The live deploy's limits (jobs count against the service's).
    let resources = spec.resources(inner).await;
    for warning in resources.warnings() {
        log.system(format!("==> Warning: {warning}"));
    }
    let mut container_spec = ContainerSpec {
        name: container_name.clone(),
        image: image.clone(),
        env: spec.job_env(),
        cmd: command.as_deref().map(instances::sh_c),
        entrypoint: None,
        labels: inner.naming.job_labels(&svc.id, &job.id),
        network: Some(inner.naming.network()),
        network_aliases: Vec::new(),
        publish: None,
        volumes: volume.into_iter().collect(),
        restart_policy: RestartPolicy::No,
        memory_limit_bytes: None,
        nano_cpus: None,
        pids_limit: None,
        log_rotation: None,
        working_dir: None,
    };
    resources.apply(&mut container_spec);
    match &command {
        Some(c) => log.system(format!("==> Running `{c}`")),
        None => log.system("==> Running the image's default command"),
    }
    if let Err(e) = mark_running(inner, &job.id).await {
        warn!(job = %job.id, "cannot mark job running: {e}");
    }
    if running.cancel.is_cancelled() {
        stopped_before_start(inner, &job.id, log, running).await;
        return;
    }
    // A second early: Docker's event timestamps are its own clock's.
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .saturating_sub(Duration::from_secs(1));
    let info = match inner.docker.run_container(&container_spec).await {
        Ok(info) => info,
        Err(e) => {
            let m = format!("starting the job container failed: {}", error_message(&e));
            log.system(format!("==> Job failed: {m}"));
            finish_job(inner, &job.id, JobStatus::Failed, None, Some(m)).await;
            return;
        }
    };

    // Output → job log, until the container stops.
    let output = {
        let mut lines = inner.docker.logs(&info.id, true, None);
        let sink = log.sink().clone();
        tokio::spawn(async move {
            while let Some(line) = lines.next().await {
                sink.send(line);
            }
        })
    };
    let mut interrupted = false;
    let exit = tokio::select! {
        r = inner.docker.wait_container(&info.id) => r,
        _ = running.cancel.cancelled() => {
            interrupted = true;
            let (_, why) = interruption(inner, running);
            log.system(format!("==> Stopping the job ({why}; grace period {JOB_STOP_GRACE_SECS}s)"));
            if let Err(e) = inner.docker.stop_container(&info.id, JOB_STOP_GRACE_SECS).await {
                warn!(job = %job.id, "stopping job container failed: {e}");
            }
            inner.docker.wait_container(&info.id).await
        }
    };
    if tokio::time::timeout(OUTPUT_DRAIN, output).await.is_err() {
        debug!(job = %job.id, "job output did not end in time");
    }
    let (status, code, error) = match exit {
        Ok(0) if !interrupted => (JobStatus::Succeeded, Some(0), None),
        Ok(code) if interrupted => {
            let (status, why) = interruption(inner, running);
            (status, Some(code), Some(why))
        }
        Ok(code) => (JobStatus::Failed, Some(code), Some(failure_reason(inner, &info.id, code, started).await)),
        Err(_) if interrupted => {
            let (status, why) = interruption(inner, running);
            (status, None, Some(why))
        }
        Err(e) => (JobStatus::Failed, None, Some(format!("waiting for the job failed: {}", error_message(&e)))),
    };
    log.system(end_line(status, error.as_deref()));
    finish_job(inner, &job.id, status, code, error).await;
    if let Err(e) = inner.docker.remove_container(&info.id, true).await {
        warn!(job = %job.id, "cannot remove job container: {e}");
    }
}

/// Why a job's container exited with a non-zero `code`: out of memory
/// (naming the limit) or just its exit code. Only a SIGKILL can be the
/// kernel's OOM killer, and Docker may record the OOM kill a moment after
/// it reports the exit (seen on Linux hosts): a SIGKILLed job's container
/// is looked at again briefly, then its Docker events since `started` (a
/// time since the epoch) are asked for as long as an instance's are — one
/// look at them right after the flag was not always late enough.
async fn failure_reason(inner: &Inner, container_id: &str, code: i64, started: Duration) -> String {
    let mut memory_limit_bytes = None;
    for attempt in 0..OOM_FLAG_POLLS {
        match inner.docker.inspect_container(container_id).await {
            Ok(Some(c)) if c.oom_killed => return failure_text(code, true, c.memory_limit_bytes),
            Ok(Some(c)) => memory_limit_bytes = c.memory_limit_bytes,
            _ => break,
        }
        if code != SIGKILL_EXIT_CODE || attempt + 1 == OOM_FLAG_POLLS {
            break;
        }
        tokio::time::sleep(OOM_FLAG_POLL).await;
    }
    let exit = LastExit { code: Some(code), oom_killed: false, memory_limit_bytes };
    let exit = crate::health::with_late_oom(inner, container_id, started, exit).await;
    failure_text(code, exit.oom_killed, memory_limit_bytes)
}

fn failure_text(code: i64, oom_killed: bool, memory_limit_bytes: Option<i64>) -> String {
    if oom_killed {
        crate::limits::oom_message("the job", memory_limit_bytes, "the service's")
    } else {
        format!("exited with code {code}")
    }
}

/// How a stopped job ends: canceled with the reason given, or failed when
/// the server is shutting down (it did not finish, nobody canceled it).
fn interruption(inner: &Inner, running: &RunningJob) -> (JobStatus, String) {
    match running.cancel_reason() {
        Some(reason) => (JobStatus::Canceled, reason),
        None if inner.shutdown.is_cancelled() => (JobStatus::Failed, SHUTDOWN.to_string()),
        None => (JobStatus::Canceled, "canceled".to_string()),
    }
}

/// Stopped before its container started.
async fn stopped_before_start(inner: &Inner, job_id: &str, log: &LogHandle, running: &RunningJob) {
    let (status, why) = interruption(inner, running);
    log.system(end_line(status, Some(&why)));
    finish_job(inner, job_id, status, None, Some(why)).await;
}

/// The last line of a job's log.
fn end_line(status: JobStatus, error: Option<&str>) -> String {
    match (status, error) {
        (JobStatus::Succeeded, _) => "==> Job succeeded".to_string(),
        (JobStatus::Canceled, Some(m)) => format!("==> Job canceled: {m}"),
        (JobStatus::Canceled, None) => "==> Job canceled".to_string(),
        (_, Some(m)) => format!("==> Job failed: {m}"),
        (_, None) => "==> Job failed".to_string(),
    }
}

async fn mark_running(inner: &Inner, job_id: &str) -> Result<()> {
    let mut job = inner.store.require_job_run(job_id).await?;
    job.status = JobStatus::Running;
    job.started_at = Some(Utc::now());
    inner.store.update_job_run(&job).await
}

/// Record the final status (before the log is finished, so a follower that
/// sees the end of the log also sees the final status). `Ok(false)` when the
/// job had already finished.
async fn record_result(
    inner: &Inner,
    job_id: &str,
    status: JobStatus,
    exit_code: Option<i64>,
    error: Option<String>,
) -> Result<bool> {
    let mut job = inner.store.require_job_run(job_id).await?;
    if job.status.is_terminal() {
        return Ok(false);
    }
    job.status = status;
    job.exit_code = exit_code;
    job.error = error;
    job.finished_at = Some(Utc::now());
    if job.started_at.is_none() {
        job.started_at = job.finished_at;
    }
    inner.store.update_job_run(&job).await?;
    info!(job = job_id, status = %status, "job finished");
    Ok(true)
}

async fn finish_job(inner: &Inner, job_id: &str, status: JobStatus, exit_code: Option<i64>, error: Option<String>) {
    if let Err(e) = record_result(inner, job_id, status, exit_code, error).await {
        warn!(job = job_id, "cannot record job result: {e}");
    }
}

/// Stop (with `grace_secs`) and remove the container(s) of a job run.
async fn remove_job_containers(inner: &Inner, job_id: &str, grace_secs: u32) {
    let prefix = inner.naming.prefix().to_string();
    match inner.docker.list_containers(&[(LABEL_INSTANCE, prefix.as_str()), (LABEL_JOB, job_id)], true).await {
        Ok(list) => {
            if let Err(e) = instances::retire(inner, &list, grace_secs).await {
                warn!(job = job_id, "cannot remove job container: {e}");
            }
        }
        Err(e) => warn!(job = job_id, "cannot list job containers: {e}"),
    }
}

/// `Engine::cancel_job`: stop a pending or running job (its container gets
/// [`JOB_STOP_GRACE_SECS`]) and record it `canceled`. Conflict once it has
/// finished.
pub(crate) async fn cancel_job(inner: &Arc<Inner>, job_id: &str) -> Result<JobRun> {
    let job = inner.store.require_job_run(job_id.trim()).await?;
    if job.status.is_terminal() {
        return Err(already_finished(&job));
    }
    match inner.with_rt(|rt| rt.jobs.get(&job.id).cloned()) {
        Some(running) => {
            info!(job = %job.id, "canceling job");
            running.request_cancel(CANCELED_BY_USER);
            if !running.wait_done(STOP_WAIT).await {
                warn!(job = %job.id, "the job did not stop within {}s of being canceled", STOP_WAIT.as_secs());
            }
            inner.store.require_job_run(&job.id).await
        }
        None => {
            // No task of this server runs it (e.g. its result could not be
            // recorded): stop whatever is left of it and record the cancel.
            remove_job_containers(inner, &job.id, JOB_STOP_GRACE_SECS).await;
            if !record_result(inner, &job.id, JobStatus::Canceled, None, Some(CANCELED_BY_USER.into())).await? {
                // It finished in the meantime.
                return Err(already_finished(&inner.store.require_job_run(&job.id).await?));
            }
            let log = inner.logs.open(LogKind::Job, &job.id);
            log.system(end_line(JobStatus::Canceled, Some(CANCELED_BY_USER)));
            log.finish().await;
            let canceled = inner.store.require_job_run(&job.id).await;
            prune_runs(inner, &job.service_id, Some(&job.id)).await;
            canceled
        }
    }
}

fn already_finished(job: &JobRun) -> Error {
    Error::conflict(format!("job {} already finished ({})", job.id, job.status))
}

/// Stop the running jobs of a service (suspend, delete: `reason` becomes
/// their error). Returns them, for [`wait_jobs_stopped`].
pub(crate) fn cancel_service_jobs(inner: &Inner, service_id: &str, reason: &str) -> Vec<RunningJob> {
    let jobs = inner.running_jobs_of(service_id);
    for j in &jobs {
        info!(service = service_id, job = %j.job_id, "stopping job: {reason}");
        j.request_cancel(reason);
    }
    jobs
}

/// Wait (bounded) for stopped jobs to be recorded.
pub(crate) async fn wait_jobs_stopped(service_id: &str, jobs: &[RunningJob]) {
    for j in jobs {
        if !j.wait_done(STOP_WAIT).await {
            warn!(service = service_id, job = %j.job_id, "a job did not stop within {}s", STOP_WAIT.as_secs());
        }
    }
}

/// Job-run retention: keep the newest `config.keep_job_runs` finished runs of
/// the service (at least one) and delete older ones with their logs.
/// `finished` (a run that just ended, its log finished) loses its log file
/// too if another run's retention deleted its row while it was still writing.
async fn prune_runs(inner: &Inner, service_id: &str, finished: Option<&str>) {
    let keep = u32::try_from(inner.config.keep_job_runs.max(1)).unwrap_or(u32::MAX);
    let pruned = match inner.store.prune_job_runs(service_id, keep).await {
        Ok(ids) => ids,
        Err(e) => {
            warn!(service = service_id, "cannot delete old job runs: {e}");
            return;
        }
    };
    for id in &pruned {
        inner.logs.remove(LogKind::Job, id).await;
    }
    if !pruned.is_empty() {
        debug!(service = service_id, count = pruned.len(), keep, "deleted old job runs and their logs");
    }
    if let Some(id) = finished
        && !pruned.iter().any(|p| p == id)
        && matches!(inner.store.get_job_run(id).await, Ok(None))
    {
        inner.logs.remove(LogKind::Job, id).await;
    }
}

/// Written into a cron run's log each time the schedule fires while it
/// still runs.
fn skipped_run_line(running_for: &str) -> String {
    format!(
        "==> Warning: a scheduled run was skipped because this run is still running (for {running_for}); \
         cancel it if it is stuck, or the schedule stays blocked"
    )
}

/// `1h 5m`, `3m 12s`, `45s`.
fn human_duration(d: Duration) -> String {
    let s = d.as_secs();
    match (s / 3600, (s % 3600) / 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

// ---------------------------------------------------------------------------
// cron

/// Decides which cron jobs fire, given the time of the previous tick.
#[derive(Debug, Clone)]
pub(crate) struct CronTicker {
    last: DateTime<Utc>,
}

impl CronTicker {
    /// Runs missed before `now` (e.g. while the server was down) are skipped.
    pub(crate) fn new(now: DateTime<Utc>) -> Self {
        CronTicker { last: now }
    }

    /// Ids of the cron services whose schedule fires in `(last tick, now]`;
    /// advances the tick. Suspended services and services without a live
    /// deploy don't run.
    pub(crate) fn due(&mut self, now: DateTime<Utc>, services: &[Service]) -> Vec<String> {
        if now <= self.last {
            // Clock went backwards (or no time passed): just re-anchor.
            self.last = now;
            return Vec::new();
        }
        let from = self.last;
        self.last = now;
        services
            .iter()
            .filter(|s| s.service_type == ServiceType::CronJob && !s.suspended && s.live_deploy_id.is_some())
            .filter(|s| match s.schedule.as_deref().map(Schedule::parse) {
                Some(Ok(schedule)) => schedule.fires_between(from, now),
                Some(Err(e)) => {
                    debug!(service = %s.name, "invalid schedule: {e}");
                    false
                }
                None => false,
            })
            .map(|s| s.id.clone())
            .collect()
    }
}

/// The scheduler loop (until shutdown).
pub(crate) async fn run_scheduler(inner: Arc<Inner>) {
    let mut ticker = CronTicker::new(Utc::now());
    loop {
        tokio::select! {
            _ = inner.shutdown.cancelled() => return,
            _ = tokio::time::sleep(TICK) => {}
        }
        let services = match inner.store.list_services().await {
            Ok(s) => s,
            Err(e) => {
                warn!("cron: cannot list services: {e}");
                continue;
            }
        };
        for id in ticker.due(Utc::now(), &services) {
            let name = services.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
            let running = inner.running_jobs_of(&id);
            if let Some(previous) = running.iter().min_by_key(|j| j.started) {
                // Runs never overlap: a hung run blocks the schedule until it
                // is canceled. Say so loudly, in the server log and the run's log.
                let since = human_duration(previous.started.elapsed());
                warn!(
                    service = %name,
                    job = %previous.job_id,
                    "cron: skipped a scheduled run: the previous run {} has been running for {since}; if it is \
                     stuck, cancel it (POST /api/v1/jobs/{}/cancel) so that the schedule runs again",
                    previous.job_id,
                    previous.job_id
                );
                if let Some(log) = inner.logs.get(LogKind::Job, &previous.job_id) {
                    log.system(skipped_run_line(&since));
                }
                continue;
            }
            match run_job(&inner, &id, None, JobTrigger::Schedule).await {
                Ok(job) => info!(service = %name, job = %job.id, "cron: started scheduled run"),
                Err(e) => warn!(service = %name, "cron: cannot start scheduled run: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 1, h, m, s).unwrap()
    }

    fn cron(name: &str, schedule: &str) -> Service {
        let mut s = Service::new(name, ServiceType::CronJob);
        s.schedule = Some(schedule.into());
        s.live_deploy_id = Some("dep-1".into());
        s
    }

    #[test]
    fn fires_once_per_matching_minute_with_15s_ticks() {
        let every5 = cron("every5", "*/5 * * * *");
        let hourly = cron("hourly", "0 * * * *");
        let services = vec![every5.clone(), hourly.clone()];
        let mut ticker = CronTicker::new(at(9, 58, 7));
        let mut fired: Vec<(DateTime<Utc>, String)> = Vec::new();
        let mut now = at(9, 58, 7);
        // 20 minutes of 15-second ticks.
        for _ in 0..80 {
            now += chrono::Duration::seconds(15);
            for id in ticker.due(now, &services) {
                fired.push((now, id));
            }
        }
        let count = |id: &str| fired.iter().filter(|(_, f)| f == id).count();
        // 10:00, 10:05, 10:10, 10:15 (the window ends at 10:18:07).
        assert_eq!(count(&every5.id), 4);
        assert_eq!(count(&hourly.id), 1);
        let first = fired.iter().find(|(_, id)| *id == hourly.id).unwrap().0;
        assert!(first >= at(10, 0, 0) && first < at(10, 0, 15), "fires on the first tick at/after 10:00");
    }

    #[test]
    fn skips_suspended_undeployed_invalid_and_missed_runs() {
        let mut suspended = cron("s", "* * * * *");
        suspended.suspended = true;
        let mut undeployed = cron("u", "* * * * *");
        undeployed.live_deploy_id = None;
        let invalid = cron("i", "not a schedule");
        let web = Service::new("web", ServiceType::WebService);
        let ok = cron("ok", "* * * * *");
        let services = vec![suspended, undeployed, invalid, web, ok.clone()];
        let mut ticker = CronTicker::new(at(10, 0, 30));
        assert_eq!(ticker.due(at(10, 1, 0), &services), vec![ok.id.clone()]);
        assert!(ticker.due(at(10, 1, 15), &services).is_empty());
        // A long pause (server busy / asleep): one run at most, not one per missed minute.
        assert_eq!(ticker.due(at(11, 0, 0), &services), vec![ok.id.clone()]);
        // Clock going backwards: nothing fires, the tick re-anchors.
        assert!(ticker.due(at(10, 30, 0), &services).is_empty());
        assert_eq!(ticker.due(at(10, 31, 0), &services), vec![ok.id]);
    }

    #[test]
    fn end_lines_and_durations() {
        assert_eq!(end_line(JobStatus::Succeeded, None), "==> Job succeeded");
        assert_eq!(end_line(JobStatus::Canceled, Some("canceled by user")), "==> Job canceled: canceled by user");
        assert_eq!(end_line(JobStatus::Failed, Some("exited with code 3")), "==> Job failed: exited with code 3");
        assert_eq!(human_duration(Duration::from_secs(45)), "45s");
        assert_eq!(human_duration(Duration::from_secs(192)), "3m 12s");
        assert_eq!(human_duration(Duration::from_secs(3900)), "1h 5m");
        assert_eq!(failure_text(3, false, None), "exited with code 3");
        assert_eq!(
            failure_text(137, true, Some(64 << 20)),
            "the job ran out of memory (limit 64 MiB) — raise the service's memory limit"
        );
        let line = skipped_run_line("3m 12s");
        assert!(line.starts_with("==> Warning:") && line.contains("cancel it"), "{line}");
    }

    #[test]
    fn a_fresh_ticker_skips_runs_missed_while_down() {
        let services = vec![cron("c", "0 0 * * *")];
        // Boot at 00:00:30: the midnight run was missed while the server was down.
        let mut ticker = CronTicker::new(at(0, 0, 30));
        assert!(ticker.due(at(0, 0, 45), &services).is_empty());
    }
}
