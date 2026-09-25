//! One-off jobs (`ferry run`) and the cron scheduler.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ferry_core::naming::{LABEL_INSTANCE, LABEL_JOB};
use ferry_core::schedule::Schedule;
use ferry_core::{Error, JobRun, JobStatus, JobTrigger, Result, Service, ServiceType};
use ferry_docker::{ContainerSpec, RestartPolicy};
use futures::StreamExt;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::instances;
use crate::logs::{LogHandle, LogKind};
use crate::state::{Inner, RunningJob};
use crate::util::{error_message, panic_message};

/// Scheduler tick interval.
pub(crate) const TICK: Duration = Duration::from_secs(15);
/// Grace period when a job is stopped (shutdown, service deleted).
const JOB_STOP_GRACE_SECS: u32 = 10;
/// How long a job's output may keep draining after its container exited.
const OUTPUT_DRAIN: Duration = Duration::from_secs(10);

/// `Engine::run_job`.
pub(crate) async fn run_job(
    inner: &Arc<Inner>,
    service_id: &str,
    command: Option<String>,
    trigger: JobTrigger,
) -> Result<JobRun> {
    let svc = inner.store.require_service(service_id).await?;
    if inner.is_service_deleting(&svc.id) {
        return Err(Error::conflict(format!("service '{}' is being deleted", svc.name)));
    }
    if svc.suspended {
        return Err(Error::conflict(format!("service '{}' is suspended", svc.name)));
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

    let mut job = JobRun::new(&svc.id, trigger, command);
    job.image = Some(image);
    inner.store.create_job_run(&job).await?;
    let log = inner.logs.open(LogKind::Job, &job.id);
    let (done_tx, done_rx) = watch::channel(false);
    let cancel = inner.shutdown.child_token();
    inner.with_rt(|rt| {
        rt.jobs.insert(job.id.clone(), RunningJob { service_id: svc.id.clone(), cancel: cancel.clone(), done: done_rx })
    });
    info!(service = %svc.name, job = %job.id, trigger = %trigger, "job started");

    let (inner2, job2) = (inner.clone(), job.clone());
    tokio::spawn(async move {
        let task = {
            let (inner, job, log, cancel) = (inner2.clone(), job2.clone(), log.clone(), cancel.clone());
            tokio::spawn(async move { execute(&inner, &svc, job, &log, &cancel).await })
        };
        if let Err(e) = task.await {
            error!(job = %job2.id, "job task failed: {e}");
            let m = panic_message(&e);
            log.system(format!("==> Job failed: {m}"));
            finish_job(&inner2, &job2.id, JobStatus::Failed, None, Some(m)).await;
            remove_job_containers(&inner2, &job2.id).await;
        }
        log.finish().await;
        inner2.with_rt(|rt| rt.jobs.remove(&job2.id));
        let _ = done_tx.send(true);
    });
    Ok(job)
}

/// Run the job's container to completion.
async fn execute(inner: &Arc<Inner>, svc: &Service, job: JobRun, log: &LogHandle, cancel: &CancellationToken) {
    let image = job.image.clone().unwrap_or_default();
    let container_name = inner.naming.job_container(&svc.name, &job.id);
    let prepared = async {
        let live_id = svc.live_deploy_id.clone().unwrap_or_default();
        let live = inner.store.require_deploy(&live_id).await?;
        let env = instances::container_env(inner, svc, &live, None).await?;
        if !inner.docker.image_exists(&image).await? {
            return Err(Error::conflict(format!("image {image} no longer exists: deploy the service again")));
        }
        Ok(env)
    }
    .await;
    let env = match prepared {
        Ok(env) => env,
        Err(e) => {
            let m = error_message(&e);
            log.system(format!("==> Job failed: {m}"));
            finish_job(inner, &job.id, JobStatus::Failed, None, Some(m)).await;
            return;
        }
    };
    let command = job.command.clone().or_else(|| svc.start_command.clone()).filter(|c| !c.trim().is_empty());
    let spec = ContainerSpec {
        name: container_name.clone(),
        image: image.clone(),
        env,
        cmd: command.as_deref().map(instances::sh_c),
        entrypoint: None,
        labels: inner.naming.job_labels(&svc.id, &job.id),
        network: Some(inner.naming.network()),
        network_aliases: Vec::new(),
        publish: None,
        volumes: Vec::new(),
        restart_policy: RestartPolicy::No,
        memory_limit_bytes: None,
        nano_cpus: None,
        working_dir: None,
    };
    match &command {
        Some(c) => log.system(format!("==> Running `{c}`")),
        None => log.system("==> Running the image's default command"),
    }
    if let Err(e) = mark_running(inner, &job.id).await {
        warn!(job = %job.id, "cannot mark job running: {e}");
    }
    let info = match inner.docker.run_container(&spec).await {
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
        _ = cancel.cancelled() => {
            interrupted = true;
            log.system("==> Stopping the job");
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
            let reason = if inner.shutdown.is_cancelled() { "interrupted by server shutdown" } else { "stopped" };
            (JobStatus::Failed, Some(code), Some(reason.to_string()))
        }
        Ok(code) => (JobStatus::Failed, Some(code), Some(format!("exited with code {code}"))),
        Err(e) => (JobStatus::Failed, None, Some(format!("waiting for the job failed: {}", error_message(&e)))),
    };
    match (&status, &error) {
        (JobStatus::Succeeded, _) => log.system("==> Job succeeded"),
        (_, Some(m)) => log.system(format!("==> Job failed: {m}")),
        _ => log.system("==> Job failed"),
    }
    finish_job(inner, &job.id, status, code, error).await;
    if let Err(e) = inner.docker.remove_container(&info.id, true).await {
        warn!(job = %job.id, "cannot remove job container: {e}");
    }
}

async fn mark_running(inner: &Inner, job_id: &str) -> Result<()> {
    let mut job = inner.store.require_job_run(job_id).await?;
    job.status = JobStatus::Running;
    job.started_at = Some(Utc::now());
    inner.store.update_job_run(&job).await
}

/// Record the final status (before the log is finished, so a follower that
/// sees the end of the log also sees the final status).
async fn finish_job(inner: &Inner, job_id: &str, status: JobStatus, exit_code: Option<i64>, error: Option<String>) {
    let res = async {
        let mut job = inner.store.require_job_run(job_id).await?;
        if job.status.is_terminal() {
            return Ok(());
        }
        job.status = status;
        job.exit_code = exit_code;
        job.error = error;
        job.finished_at = Some(Utc::now());
        if job.started_at.is_none() {
            job.started_at = job.finished_at;
        }
        inner.store.update_job_run(&job).await
    }
    .await;
    match res {
        Ok(()) => info!(job = job_id, status = %status, "job finished"),
        Err(e) => warn!(job = job_id, "cannot record job result: {e}"),
    }
}

/// Remove the container(s) of a job run.
async fn remove_job_containers(inner: &Inner, job_id: &str) {
    let prefix = inner.naming.prefix().to_string();
    match inner.docker.list_containers(&[(LABEL_INSTANCE, prefix.as_str()), (LABEL_JOB, job_id)], true).await {
        Ok(list) => {
            if let Err(e) = instances::retire(inner, &list, 0).await {
                warn!(job = job_id, "cannot remove job container: {e}");
            }
        }
        Err(e) => warn!(job = job_id, "cannot list job containers: {e}"),
    }
}

/// Stop the running jobs of a service and wait for them (service deletion).
pub(crate) async fn stop_service_jobs(inner: &Inner, service_id: &str) {
    let jobs = inner.running_jobs_of(service_id);
    for j in &jobs {
        j.cancel.cancel();
    }
    for j in &jobs {
        if !j.wait_done(Duration::from_secs(u64::from(JOB_STOP_GRACE_SECS) + 20)).await {
            warn!(service = service_id, "a job did not stop in time");
        }
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
            if !inner.running_jobs_of(&id).is_empty() {
                info!(service = %name, "cron: skipping this run, the previous run is still running");
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
    fn a_fresh_ticker_skips_runs_missed_while_down() {
        let services = vec![cron("c", "0 0 * * *")];
        // Boot at 00:00:30: the midnight run was missed while the server was down.
        let mut ticker = CronTicker::new(at(0, 0, 30));
        assert!(ticker.due(at(0, 0, 45), &services).is_empty());
    }
}
