//! Runtime status (instances + CPU/memory) and runtime logs of a service.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ferry_core::dto::{InstanceStatus, RuntimeStatus};
use ferry_core::naming::LABEL_DEPLOY;
use ferry_core::resources::round_cpus;
use ferry_core::{LogLine, LogOptions, LogStream, Result, compute_service_state};
use ferry_docker::ContainerInfo;
use futures::StreamExt;
use futures::future::join_all;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::instances;
use crate::state::Inner;
use crate::util::{instance_id, lock, with_timeout};

/// Bounds on Docker calls made while answering the API (never hang it).
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const INSPECT_TIMEOUT: Duration = Duration::from_secs(3);
const STATS_TIMEOUT: Duration = Duration::from_secs(4);
/// How often a followed log stream looks for new containers.
const DISCOVER_INTERVAL: Duration = Duration::from_secs(2);
/// Lines requested when re-attaching to a container that restarted (older
/// ones already sent are filtered out by timestamp).
const REATTACH_TAIL: usize = 1000;
const FOLLOW_BUFFER: usize = 1024;

/// `Engine::service_status`.
pub(crate) async fn service_status(inner: &Arc<Inner>, service_id: &str) -> Result<RuntimeStatus> {
    let svc = inner.store.require_service(service_id).await?;
    let latest = inner.store.latest_deploy(&svc.id).await?;
    let containers =
        with_timeout("listing containers", LIST_TIMEOUT, instances::service_containers(inner, &svc.id, true)).await?;
    let instances: Vec<InstanceStatus> = join_all(containers.iter().map(|c| instance_status(inner, c))).await;
    let live = svc.live_deploy_id.as_deref();
    let running_live =
        instances.iter().filter(|i| i.state == "running" && live.is_some() && i.deploy_id.as_deref() == live).count();
    let state = compute_service_state(&svc, latest.as_ref(), Some(u32::try_from(running_live).unwrap_or(u32::MAX)));
    Ok(RuntimeStatus { service_id: svc.id.clone(), state, desired_instances: svc.desired_instances(), instances })
}

async fn instance_status(inner: &Inner, c: &ContainerInfo) -> InstanceStatus {
    // The list API has no start time, restart count, limits or OOM flag:
    // inspect (bounded).
    let info = match tokio::time::timeout(INSPECT_TIMEOUT, inner.docker.inspect_container(&c.id)).await {
        Ok(Ok(Some(info))) => info,
        _ => c.clone(),
    };
    let stats = if info.state.is_running() {
        match tokio::time::timeout(STATS_TIMEOUT, inner.docker.stats(&c.id)).await {
            Ok(Ok(s)) => Some(s),
            _ => None,
        }
    } else {
        None
    };
    to_status(&info, stats)
}

fn to_status(info: &ContainerInfo, stats: Option<ferry_docker::ContainerStats>) -> InstanceStatus {
    InstanceStatus {
        container_id: info.id.clone(),
        name: info.name.clone(),
        deploy_id: info.labels.get(LABEL_DEPLOY).cloned(),
        state: info.state.as_str().to_string(),
        host_port: info.host_port,
        started_at: info.started_at.clone(),
        restart_count: info.restart_count,
        cpu_percent: stats.map(|s| s.cpu_percent),
        memory_bytes: stats.map(|s| s.memory_bytes),
        // The configured limit (docker stats reports the host's memory for
        // unlimited containers).
        memory_limit_bytes: info.memory_limit_bytes.and_then(|l| u64::try_from(l).ok()).filter(|l| *l > 0),
        cpu_limit: info.nano_cpus.filter(|n| *n > 0).map(|n| round_cpus(n as f64 / 1e9)),
        oom_killed: info.oom_killed,
        exit_code: info.exit_code,
    }
}

/// The containers whose logs make up the service's runtime logs: those of
/// the live deploy, or all of them when there is no live deploy yet.
fn current_containers(containers: &[ContainerInfo], live: Option<&str>) -> Vec<ContainerInfo> {
    let of_live: Vec<ContainerInfo> = containers
        .iter()
        .filter(|c| live.is_some() && c.labels.get(LABEL_DEPLOY).map(String::as_str) == live)
        .cloned()
        .collect();
    if of_live.is_empty() { containers.to_vec() } else { of_live }
}

/// `Engine::service_logs`.
pub(crate) async fn service_logs(inner: &Arc<Inner>, service_id: &str, opts: LogOptions) -> Result<LogStream> {
    let svc = inner.store.require_service(service_id).await?;
    let containers =
        with_timeout("listing containers", LIST_TIMEOUT, instances::service_containers(inner, &svc.id, true)).await?;
    let current = current_containers(&containers, svc.live_deploy_id.as_deref());
    if !opts.follow {
        let per_instance = join_all(current.iter().map(|c| {
            let instance = instance_id(&c.name);
            inner
                .docker
                .logs(&c.id, false, opts.tail)
                .map(move |l| l.with_instance(instance.clone()))
                .collect::<Vec<_>>()
        }))
        .await;
        let mut lines: Vec<LogLine> = per_instance.into_iter().flatten().collect();
        lines.sort_by_key(|l| l.ts);
        return Ok(Box::pin(futures::stream::iter(lines)));
    }
    let (tx, rx) = mpsc::channel(FOLLOW_BUFFER);
    tokio::spawn(follow_service(inner.clone(), svc.id.clone(), current, opts.tail, tx));
    Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
}

/// One followed container.
struct Attached {
    task: JoinHandle<()>,
    /// Timestamp of the last line forwarded (to skip it on re-attach).
    last_ts: Arc<StdMutex<Option<DateTime<Utc>>>>,
}

/// Merge the followed output of the service's containers until the client
/// goes away (or the server shuts down), attaching to new containers
/// (redeploys, scaling, replacements) as they appear.
async fn follow_service(
    inner: Arc<Inner>,
    service_id: String,
    initial: Vec<ContainerInfo>,
    tail: Option<usize>,
    tx: mpsc::Sender<LogLine>,
) {
    let mut attached: HashMap<String, Attached> = HashMap::new();
    for c in &initial {
        attach(&inner, c, tail, None, &tx, &mut attached);
    }
    loop {
        tokio::select! {
            _ = tx.closed() => break,
            _ = inner.shutdown.cancelled() => break,
            _ = tokio::time::sleep(DISCOVER_INTERVAL) => {}
        }
        match inner.store.get_service(&service_id).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => continue,
        }
        let Ok(running) = instances::service_containers(&inner, &service_id, false).await else { continue };
        for c in &running {
            match attached.get(&c.id) {
                None => attach(&inner, c, None, None, &tx, &mut attached),
                // Its stream ended (the container stopped) but it runs again.
                Some(a) if a.task.is_finished() => {
                    let since = *lock(&a.last_ts);
                    attach(&inner, c, Some(REATTACH_TAIL), since, &tx, &mut attached);
                }
                Some(_) => {}
            }
        }
    }
    for (_, a) in attached {
        a.task.abort();
    }
}

fn attach(
    inner: &Inner,
    c: &ContainerInfo,
    tail: Option<usize>,
    since: Option<DateTime<Utc>>,
    tx: &mpsc::Sender<LogLine>,
    attached: &mut HashMap<String, Attached>,
) {
    let mut lines = inner.docker.logs(&c.id, true, tail);
    let instance = instance_id(&c.name);
    let last_ts: Arc<StdMutex<Option<DateTime<Utc>>>> = Arc::new(StdMutex::new(since));
    let (tx, last) = (tx.clone(), last_ts.clone());
    let task = tokio::spawn(async move {
        while let Some(line) = lines.next().await {
            if since.is_some_and(|s| line.ts <= s) {
                continue;
            }
            *lock(&last) = Some(line.ts);
            if tx.send(line.with_instance(instance.clone())).await.is_err() {
                break;
            }
        }
    });
    if let Some(old) = attached.insert(c.id.clone(), Attached { task, last_ts }) {
        old.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_docker::ContainerState;
    use std::collections::BTreeMap;

    fn c(name: &str, deploy: &str) -> ContainerInfo {
        ContainerInfo {
            id: name.into(),
            name: name.into(),
            image: "i".into(),
            state: ContainerState::Running,
            exit_code: None,
            host_port: None,
            labels: BTreeMap::from([(LABEL_DEPLOY.to_string(), deploy.to_string())]),
            started_at: None,
            restart_count: None,
            oom_killed: false,
            memory_limit_bytes: None,
            nano_cpus: None,
        }
    }

    #[test]
    fn instance_status_reports_the_configured_limits_and_oom_kills() {
        let mut info = c("a", "live");
        info.memory_limit_bytes = Some(512 << 20);
        info.nano_cpus = Some(1_500_000_000);
        info.state = ContainerState::Restarting;
        info.oom_killed = true;
        info.exit_code = Some(137);
        let st = to_status(&info, None);
        assert_eq!(st.memory_limit_bytes, Some(512 << 20));
        assert_eq!(st.cpu_limit, Some(1.5));
        assert!(st.oom_killed);
        assert_eq!(st.exit_code, Some(137));
        assert_eq!(st.state, "restarting");
        // Unlimited: no limit, whatever docker stats says.
        let stats = ferry_docker::ContainerStats { cpu_percent: 1.0, memory_bytes: 10, memory_limit_bytes: 8 << 30 };
        let st = to_status(&c("b", "live"), Some(stats));
        assert_eq!((st.memory_limit_bytes, st.cpu_limit, st.oom_killed), (None, None, false));
        assert_eq!(st.memory_bytes, Some(10));
    }

    #[test]
    fn current_containers_prefer_the_live_deploy() {
        let all = vec![c("a", "live"), c("b", "new"), c("c", "live")];
        let names = |v: Vec<ContainerInfo>| v.into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(current_containers(&all, Some("live"))), ["a", "c"]);
        assert_eq!(names(current_containers(&all, None)), ["a", "b", "c"]);
        assert_eq!(names(current_containers(&all, Some("gone"))), ["a", "b", "c"]);
    }
}
