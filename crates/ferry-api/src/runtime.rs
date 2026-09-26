//! Recently observed running-instance counts, so service views can say
//! `degraded` instead of `live` when a live service's containers are down.
//!
//! The stored row only knows that a deploy went live; how many of its
//! containers still run is known to the engine (`Engine::service_status`,
//! which inspects Docker and samples stats — too slow to call for every row
//! of every listing). So counts are cached per service for [`FRESH_FOR`]:
//! every status call feeds the cache, and the list / show handlers refresh
//! stale entries of live services concurrently, waiting at most
//! [`WAIT_AT_MOST`] (a slow Docker daemon makes the view fall back to the
//! last observation, or to the stored state).
//!
//! Process-wide like [`crate::locks`]: service ids are unique across stores.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ferry_core::dto::RuntimeStatus;
use ferry_core::{Engine, Service};

/// An observation younger than this isn't refreshed.
const FRESH_FOR: Duration = Duration::from_secs(10);

/// An observation older than this isn't used at all.
const USABLE_FOR: Duration = Duration::from_secs(60);

/// How long a view waits for refreshed counts.
const WAIT_AT_MOST: Duration = Duration::from_millis(2500);

struct Observation {
    /// The live deploy the count refers to.
    live_deploy_id: Option<String>,
    running: u32,
    at: Instant,
}

#[derive(Default)]
struct Cache {
    seen: HashMap<String, Observation>,
    /// Services with a refresh in flight (at most one each).
    refreshing: HashSet<String>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

fn cache() -> MutexGuard<'static, Cache> {
    // A poisoned cache is still a valid cache (no invariant spans the panic).
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Running containers of the live deploy in an engine status report.
fn running_live(status: &RuntimeStatus, live_deploy_id: Option<&str>) -> u32 {
    let Some(live) = live_deploy_id else { return 0 };
    let n = status.instances.iter().filter(|i| i.state == "running" && i.deploy_id.as_deref() == Some(live)).count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Remember what the engine reported for `service` (read before the call).
pub fn record(service: &Service, status: &RuntimeStatus) {
    let running = running_live(status, service.live_deploy_id.as_deref());
    let mut c = cache();
    c.seen.retain(|_, o| o.at.elapsed() <= USABLE_FOR);
    c.seen.insert(
        service.id.clone(),
        Observation { live_deploy_id: service.live_deploy_id.clone(), running, at: Instant::now() },
    );
}

/// Running instances of the service's live deploy, when observed recently.
pub fn running(service: &Service) -> Option<u32> {
    let c = cache();
    let o = c.seen.get(&service.id)?;
    (o.at.elapsed() <= USABLE_FOR && o.live_deploy_id == service.live_deploy_id).then_some(o.running)
}

/// Whether a view of `service` can use a count at all.
fn wants_count(service: &Service) -> bool {
    service.live_deploy_id.is_some() && !service.suspended
}

/// Removes the in-flight mark even if the refresh task panics.
struct Refreshing(String);

impl Drop for Refreshing {
    fn drop(&mut self) {
        cache().refreshing.remove(&self.0);
    }
}

/// Refresh the counts of the live services among `services` that weren't
/// observed recently, waiting at most [`WAIT_AT_MOST`] for the answers
/// (slower ones still land in the cache for the next view).
pub async fn refresh(engine: &Arc<dyn Engine>, services: &[Service]) {
    let mut tasks = Vec::new();
    {
        let mut c = cache();
        for svc in services.iter().filter(|s| wants_count(s)) {
            let fresh = c
                .seen
                .get(&svc.id)
                .is_some_and(|o| o.at.elapsed() < FRESH_FOR && o.live_deploy_id == svc.live_deploy_id);
            if fresh || !c.refreshing.insert(svc.id.clone()) {
                continue;
            }
            let (engine, svc) = (engine.clone(), svc.clone());
            tasks.push(tokio::spawn(async move {
                let _mark = Refreshing(svc.id.clone());
                match engine.service_status(&svc.id).await {
                    Ok(status) => record(&svc, &status),
                    Err(e) => tracing::debug!(service = %svc.name, "reading the runtime status failed: {e}"),
                }
            }));
        }
    }
    if !tasks.is_empty() {
        let _ = tokio::time::timeout(WAIT_AT_MOST, futures::future::join_all(tasks)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::ServiceType;
    use ferry_core::dto::InstanceStatus;

    fn instance(deploy: &str, state: &str) -> InstanceStatus {
        InstanceStatus {
            container_id: "c".into(),
            name: "n".into(),
            deploy_id: Some(deploy.into()),
            state: state.into(),
            host_port: None,
            started_at: None,
            restart_count: None,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: None,
        }
    }

    #[test]
    fn counts_running_containers_of_the_live_deploy_only() {
        let mut svc = Service::new("runtime-cache-test", ServiceType::WebService);
        assert_eq!(running(&svc), None);
        svc.live_deploy_id = Some("dep-live".into());
        let status = RuntimeStatus {
            service_id: svc.id.clone(),
            state: ferry_core::ServiceState::Live,
            desired_instances: 3,
            instances: vec![
                instance("dep-live", "running"),
                instance("dep-live", "exited"),
                instance("dep-old", "running"),
                instance("dep-live", "running"),
            ],
        };
        record(&svc, &status);
        assert_eq!(running(&svc), Some(2));
        // another live deploy since: the observation doesn't apply
        svc.live_deploy_id = Some("dep-newer".into());
        assert_eq!(running(&svc), None);
    }
}
