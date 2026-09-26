//! `GET /api/v1/events` — the change feed of the web client (SSE).
//!
//! ```text
//! event: ready
//! data: {}
//!
//! event: change
//! data: {"kind":"deploy","id":"dep-…","service_id":"srv-…","action":"updated"}
//! ```
//!
//! `kind` is `service`, `deploy`, `datastore`, `env_group` or `job`; `action`
//! is `created`, `updated` or `deleted`; `service_id` is set for services
//! (their own id), deploys and jobs, `null` otherwise. A subscriber that falls
//! behind gets `{"kind":"all","id":"*","service_id":null,"action":"resync"}`
//! and should refetch everything. `ready` is sent once the feed watches the
//! store: changes after it are reported, so a client (re)fetches its data
//! after `ready`. Keep-alive comments every 15s; the stream ends when the
//! server shuts down.
//!
//! Changes come from the API, but also from the engine (deploy progress,
//! datastore provisioning, cron runs), which writes the store directly — so
//! the feed **polls** the store: one shared detector task (started with the
//! first subscriber, stopped when the last one leaves) fingerprints every
//! service (row, own variables, env group links), datastore, env group (row,
//! variables, linked services) and the recent / active deploys and job runs
//! every [`POLL`], and broadcasts the differences. Every mutating request
//! also nudges it, so API-driven changes are reported at once.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Extension;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use bytes::Bytes;
use ferry_core::{CancellationToken, Result, Store};
use http::Method;
use serde::Serialize;
use sqlx::Row;
use tokio::sync::{Notify, broadcast, watch};

use crate::AppState;
use crate::sse;

/// How often the store is compared while somebody listens.
pub const POLL: Duration = Duration::from_secs(1);

/// Changes buffered per subscriber before it counts as lagging.
const CAPACITY: usize = 1024;

/// Deploys / job runs compared besides the active ones (newest first).
const WINDOW: i64 = 100;

/// The `ready` frame (with a data field, so `EventSource` dispatches it).
const READY_FRAME: &[u8] = b"event: ready\ndata: {}\n\n";

/// One change: the `data` of an `event: change` frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[schema(
    as = ChangeEvent,
    example = json!({"kind": "deploy", "id": "dep-01j9", "service_id": "srv-01j8", "action": "updated"})
)]
pub struct Change {
    #[schema(value_type = crate::openapi::ChangeKind)]
    pub kind: &'static str,
    /// Id of the changed resource (`*` with `resync`).
    pub id: String,
    /// Set for services (their own id), deploys and jobs (their service); `null` otherwise.
    pub service_id: Option<String>,
    #[schema(value_type = crate::openapi::ChangeAction)]
    pub action: &'static str,
}

impl Change {
    fn new(kind: &'static str, id: &str, service_id: Option<&str>, action: &'static str) -> Self {
        Change { kind, id: id.to_string(), service_id: service_id.map(str::to_string), action }
    }

    /// "You missed changes: refetch everything."
    pub fn resync() -> Self {
        Change { kind: "all", id: "*".into(), service_id: None, action: "resync" }
    }

    fn frame(&self) -> Bytes {
        let json = serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"kind":"all","id":"*","service_id":null,"action":"resync"}"#.into());
        Bytes::from(format!("event: change\ndata: {json}\n\n"))
    }
}

/// A running detector.
struct Running {
    tx: broadcast::Sender<Change>,
    /// Becomes true once the first snapshot is taken.
    ready: watch::Receiver<bool>,
}

/// The change detector shared by every subscriber of one router.
pub struct Hub {
    store: Store,
    shutdown: CancellationToken,
    nudge: Notify,
    running: Mutex<Option<Running>>,
}

impl Hub {
    pub fn new(store: Store, shutdown: CancellationToken) -> Arc<Self> {
        Arc::new(Hub { store, shutdown, nudge: Notify::new(), running: Mutex::new(None) })
    }

    fn lock(&self) -> MutexGuard<'_, Option<Running>> {
        // A poisoned slot is still a valid slot (no invariant spans the panic).
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Compare the store now (after a mutating request) instead of at the next tick.
    pub fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// Whether a detector task is running (it stops without subscribers).
    pub fn is_running(&self) -> bool {
        self.lock().is_some()
    }

    /// Subscribe, starting the detector if needed.
    fn subscribe(self: &Arc<Self>) -> (broadcast::Receiver<Change>, watch::Receiver<bool>) {
        let mut slot = self.lock();
        if let Some(r) = slot.as_ref() {
            return (r.tx.subscribe(), r.ready.clone());
        }
        let (tx, rx) = broadcast::channel(CAPACITY);
        let (ready_tx, ready_rx) = watch::channel(false);
        *slot = Some(Running { tx: tx.clone(), ready: ready_rx.clone() });
        tokio::spawn(self.clone().detect(tx, ready_tx));
        (rx, ready_rx)
    }

    async fn detect(self: Arc<Self>, tx: broadcast::Sender<Change>, ready: watch::Sender<bool>) {
        tracing::debug!("change feed: detector started");
        let mut interval = tokio::time::interval(POLL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut previous: Option<Snapshot> = None;
        loop {
            {
                // Checked under the lock `subscribe` holds while subscribing:
                // a new subscriber either joins this detector or starts the next.
                let mut slot = self.lock();
                if tx.receiver_count() == 0 || self.shutdown.is_cancelled() {
                    *slot = None;
                    tracing::debug!("change feed: detector stopped");
                    return;
                }
            }
            match Snapshot::take(&self.store).await {
                Ok(next) => {
                    if let Some(prev) = &previous {
                        for change in prev.diff(&next) {
                            // Err = no subscriber left: noticed above.
                            let _ = tx.send(change);
                        }
                    }
                    previous = Some(next);
                    ready.send_if_modified(|r| !std::mem::replace(r, true));
                }
                Err(e) => tracing::warn!("change feed: reading the store failed: {e}"),
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => {}
                _ = interval.tick() => {}
                _ = self.nudge.notified() => {}
            }
        }
    }
}

/// Middleware: nudge the detector after every mutating request.
pub async fn nudge_after_writes(State(hub): State<Arc<Hub>>, req: Request, next: Next) -> Response {
    let writes = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let resp = next.run(req).await;
    if writes {
        hub.nudge();
    }
    resp
}

/// `GET /api/v1/events`
#[utoipa::path(
    get,
    path = "/api/v1/events",
    tag = "events",
    operation_id = "streamEvents",
    summary = "Change feed (SSE)",
    description = "Tells a client what changed so it refetches only that: API writes are reported at once, the engine's own changes (deploy progress, datastore provisioning, cron runs) within about a second.",
    responses(
        (status = 200, description = crate::openapi::SSE_EVENTS, content_type = "text/event-stream", body = String,
            example = "event: ready\ndata: {}\n\nevent: change\ndata: {\"kind\":\"deploy\",\"id\":\"dep-01j9\",\"service_id\":\"srv-01j8\",\"action\":\"updated\"}\n\n"),
    ),
)]
pub async fn stream(State(st): State<AppState>, Extension(hub): Extension<Arc<Hub>>) -> Response {
    let (changes, ready) = hub.subscribe();
    sse::stream_response(frames(changes, ready, st.shutdown.clone()))
}

enum Phase {
    WaitingReady,
    Streaming,
}

/// The SSE frames of one subscriber.
fn frames(
    changes: broadcast::Receiver<Change>,
    ready: watch::Receiver<bool>,
    shutdown: CancellationToken,
) -> impl futures::Stream<Item = std::result::Result<Bytes, Infallible>> + Send + 'static {
    let state = (changes, ready, shutdown, Phase::WaitingReady);
    futures::stream::unfold(state, |(mut changes, mut ready, shutdown, phase)| async move {
        let keep_alive = tokio::time::sleep(sse::KEEP_ALIVE);
        tokio::pin!(keep_alive);
        let frame = match phase {
            Phase::WaitingReady => {
                // None: keep-alive; Some(false): the detector stopped before
                // its first snapshot (shutdown).
                let is_ready = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return None,
                    r = ready.wait_for(|r| *r) => Some(r.is_ok()),
                    _ = &mut keep_alive => None,
                };
                match is_ready {
                    Some(true) => {
                        let frame = Bytes::from_static(READY_FRAME);
                        return Some((Ok(frame), (changes, ready, shutdown, Phase::Streaming)));
                    }
                    Some(false) => return None,
                    None => Bytes::from_static(sse::KEEP_ALIVE_FRAME),
                }
            }
            Phase::Streaming => tokio::select! {
                biased;
                _ = shutdown.cancelled() => return None,
                r = changes.recv() => match r {
                    Ok(change) => change.frame(),
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::debug!(missed, "change feed: slow subscriber, asking it to resync");
                        Change::resync().frame()
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
                _ = &mut keep_alive => Bytes::from_static(sse::KEEP_ALIVE_FRAME),
            },
        };
        Some((Ok(frame), (changes, ready, shutdown, phase)))
    })
}

// ---------------------------------------------------------------------------
// snapshots

fn fingerprint(parts: &[&dyn ErasedHash]) -> u64 {
    let mut h = DefaultHasher::new();
    for p in parts {
        p.hash_into(&mut h);
    }
    h.finish()
}

/// `Hash` for heterogeneous slices.
trait ErasedHash {
    fn hash_into(&self, h: &mut DefaultHasher);
}

impl<T: Hash> ErasedHash for T {
    fn hash_into(&self, h: &mut DefaultHasher) {
        self.hash(h);
    }
}

/// Recent and active rows of `deploys` / `job_runs`.
#[derive(Debug, Default, Clone, PartialEq)]
struct Window {
    /// id → (service id, status, created_at).
    rows: HashMap<String, (String, String, String)>,
    /// The window was full: rows older than `oldest` may slide in later.
    oldest: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct Snapshot {
    services: HashMap<String, u64>,
    datastores: HashMap<String, u64>,
    env_groups: HashMap<String, u64>,
    deploys: Window,
    jobs: Window,
}

impl Snapshot {
    /// Read everything in one read transaction: a consistent view, so a
    /// change is never reported before the changes it depends on (a deploy
    /// before its service).
    async fn take(store: &Store) -> Result<Snapshot> {
        let mut tx = store.pool().begin().await?;
        // Variables per owner (service or env group), hashed.
        let mut env: BTreeMap<String, DefaultHasher> = BTreeMap::new();
        for row in
            sqlx::query("SELECT owner_id, key, value FROM env_vars ORDER BY owner_id, key").fetch_all(&mut *tx).await?
        {
            let owner: String = row.try_get("owner_id")?;
            let h = env.entry(owner).or_default();
            row.try_get::<String, _>("key")?.hash(h);
            row.try_get::<String, _>("value")?.hash(h);
        }
        let env: HashMap<String, u64> = env.into_iter().map(|(k, h)| (k, h.finish())).collect();
        // Env group links, in link order.
        let mut groups_of: HashMap<String, Vec<String>> = HashMap::new();
        let mut members_of: HashMap<String, Vec<String>> = HashMap::new();
        for row in sqlx::query("SELECT service_id, group_id FROM service_env_groups ORDER BY service_id, position")
            .fetch_all(&mut *tx)
            .await?
        {
            let (service, group): (String, String) = (row.try_get("service_id")?, row.try_get("group_id")?);
            groups_of.entry(service.clone()).or_default().push(group.clone());
            members_of.entry(group).or_default().push(service);
        }
        for members in members_of.values_mut() {
            members.sort();
        }
        let services = rows_by_id(&mut tx, "services").await?;
        let datastores = rows_by_id(&mut tx, "datastores").await?;
        let env_groups = rows_by_id(&mut tx, "env_groups").await?;
        let deploys = window(&mut tx, "deploys", "('queued', 'building', 'deploying')").await?;
        let jobs = window(&mut tx, "job_runs", "('pending', 'running')").await?;
        tx.rollback().await?;

        let none: Vec<String> = Vec::new();
        let services = services
            .into_iter()
            .map(|(id, row)| {
                let fp = fingerprint(&[&row, &env.get(&id), groups_of.get(&id).unwrap_or(&none)]);
                (id, fp)
            })
            .collect();
        let env_groups = env_groups
            .into_iter()
            .map(|(id, row)| {
                let fp = fingerprint(&[&row, &env.get(&id), members_of.get(&id).unwrap_or(&none)]);
                (id, fp)
            })
            .collect();
        Ok(Snapshot { services, datastores, env_groups, deploys, jobs })
    }

    /// What changed from `self` to `next`, in a stable order: env groups,
    /// datastores, services, deploys, jobs; creations and updates before
    /// deletions.
    fn diff(&self, next: &Snapshot) -> Vec<Change> {
        let mut out = Vec::new();
        let mut deleted = Vec::new();
        for (kind, before, after) in [
            ("env_group", &self.env_groups, &next.env_groups),
            ("datastore", &self.datastores, &next.datastores),
            ("service", &self.services, &next.services),
        ] {
            let service_id = |id: &str| (kind == "service").then(|| id.to_string());
            let mut ids: Vec<&String> = after.keys().collect();
            ids.sort();
            for id in ids {
                let action = match before.get(id) {
                    None => "created",
                    Some(fp) if *fp != after[id] => "updated",
                    Some(_) => continue,
                };
                out.push(Change { kind, id: id.clone(), service_id: service_id(id), action });
            }
            let mut gone: Vec<&String> = before.keys().filter(|id| !after.contains_key(*id)).collect();
            gone.sort();
            for id in gone {
                deleted.push(Change { kind, id: id.clone(), service_id: service_id(id), action: "deleted" });
            }
        }
        for (kind, before, after) in [("deploy", &self.deploys, &next.deploys), ("job", &self.jobs, &next.jobs)] {
            let mut ids: Vec<&String> = after.rows.keys().collect();
            ids.sort_by(|a, b| after.rows[*a].2.cmp(&after.rows[*b].2).then(a.cmp(b)));
            for id in ids {
                let (service_id, status, created_at) = &after.rows[id];
                let action = match before.rows.get(id) {
                    Some((_, old, _)) if old == status => continue,
                    Some(_) => "updated",
                    // An older row that slid into the window isn't new.
                    None if before.oldest.as_ref().is_some_and(|o| created_at < o) => continue,
                    None => "created",
                };
                out.push(Change::new(kind, id, Some(service_id), action));
            }
            // Rows leaving the window aren't reported: they were pruned, or
            // deleted with their service (which is reported).
        }
        out.extend(deleted);
        out
    }
}

type Tx = sqlx::Transaction<'static, sqlx::Sqlite>;

/// Every row of `table` as id → hash of all its columns (whatever they are).
async fn rows_by_id(tx: &mut Tx, table: &str) -> Result<HashMap<String, u64>> {
    let mut out = HashMap::new();
    for row in sqlx::query(&format!("SELECT * FROM {table}")).fetch_all(&mut **tx).await? {
        let mut h = DefaultHasher::new();
        for i in 0..row.len() {
            // SQLite hands out any value as bytes (numbers as their text).
            row.try_get_unchecked::<Option<Vec<u8>>, _>(i)?.hash(&mut h);
        }
        out.insert(row.try_get("id")?, h.finish());
    }
    Ok(out)
}

/// The newest [`WINDOW`] rows of `table` plus its active ones.
async fn window(tx: &mut Tx, table: &str, active: &str) -> Result<Window> {
    let sql = format!(
        "SELECT id, service_id, status, created_at FROM {table}
         WHERE id IN (SELECT id FROM {table} ORDER BY created_at DESC, rowid DESC LIMIT ?1)
            OR status IN {active}"
    );
    let rows = sqlx::query(&sql).bind(WINDOW).fetch_all(&mut **tx).await?;
    let mut out = Window::default();
    let mut oldest: Option<String> = None;
    let full = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= WINDOW;
    for row in rows {
        let (id, service_id, status, created_at): (String, String, String, String) =
            (row.try_get("id")?, row.try_get("service_id")?, row.try_get("status")?, row.try_get("created_at")?);
        if oldest.as_ref().is_none_or(|o| &created_at < o) {
            oldest = Some(created_at.clone());
        }
        out.rows.insert(id, (service_id, status, created_at));
    }
    // Only a full window can have older rows slide in.
    out.oldest = oldest.filter(|_| full);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn text(b: &Bytes) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    #[test]
    fn diffs() {
        let mut a = Snapshot::default();
        a.services.insert("srv-a".into(), 1);
        a.services.insert("srv-gone".into(), 1);
        a.datastores.insert("ds-1".into(), 1);
        a.deploys.rows.insert("dep-1".into(), ("srv-a".into(), "queued".into(), "2026-01-01T00:00:01Z".into()));
        let mut b = a.clone();
        b.services.insert("srv-a".into(), 2);
        b.services.remove("srv-gone");
        b.services.insert("srv-new".into(), 1);
        b.env_groups.insert("grp-1".into(), 1);
        b.deploys.rows.insert("dep-1".into(), ("srv-a".into(), "building".into(), "2026-01-01T00:00:01Z".into()));
        b.deploys.rows.insert("dep-2".into(), ("srv-new".into(), "queued".into(), "2026-01-01T00:00:02Z".into()));
        b.jobs.rows.insert("job-1".into(), ("srv-a".into(), "pending".into(), "2026-01-01T00:00:03Z".into()));
        let changes = a.diff(&b);
        let got: Vec<(&str, &str, Option<&str>, &str)> =
            changes.iter().map(|c| (c.kind, c.id.as_str(), c.service_id.as_deref(), c.action)).collect();
        assert_eq!(
            got,
            vec![
                ("env_group", "grp-1", None, "created"),
                ("service", "srv-a", Some("srv-a"), "updated"),
                ("service", "srv-new", Some("srv-new"), "created"),
                ("deploy", "dep-1", Some("srv-a"), "updated"),
                ("deploy", "dep-2", Some("srv-new"), "created"),
                ("job", "job-1", Some("srv-a"), "created"),
                ("service", "srv-gone", Some("srv-gone"), "deleted"),
            ]
        );
        assert!(b.diff(&b).is_empty());

        // rows sliding into a full window aren't new
        let mut full = Snapshot::default();
        full.deploys.oldest = Some("2026-01-01T00:00:05Z".into());
        let mut next = full.clone();
        next.deploys.rows.insert("dep-old".into(), ("s".into(), "live".into(), "2026-01-01T00:00:04Z".into()));
        next.deploys.rows.insert("dep-new".into(), ("s".into(), "queued".into(), "2026-01-01T00:00:06Z".into()));
        let ids: Vec<String> = full.diff(&next).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["dep-new"]);
    }

    #[test]
    fn change_frames_are_json() {
        let c = Change::new("deploy", "dep-1", Some("srv-1"), "created");
        assert_eq!(
            text(&c.frame()),
            "event: change\ndata: {\"kind\":\"deploy\",\"id\":\"dep-1\",\"service_id\":\"srv-1\",\"action\":\"created\"}\n\n"
        );
        assert_eq!(
            text(&Change::resync().frame()),
            "event: change\ndata: {\"kind\":\"all\",\"id\":\"*\",\"service_id\":null,\"action\":\"resync\"}\n\n"
        );
    }

    #[tokio::test]
    async fn ready_first_then_changes_and_a_resync_after_lagging() {
        let (tx, rx) = broadcast::channel(2);
        let (ready_tx, ready_rx) = watch::channel(false);
        let shutdown = CancellationToken::new();
        let mut s = Box::pin(frames(rx, ready_rx, shutdown.clone()));
        // nothing before ready
        assert!(tokio::time::timeout(Duration::from_millis(50), s.next()).await.is_err());
        ready_tx.send(true).unwrap();
        assert_eq!(&s.next().await.unwrap().unwrap()[..], READY_FRAME);
        tx.send(Change::new("service", "srv-1", Some("srv-1"), "created")).unwrap();
        assert!(text(&s.next().await.unwrap().unwrap()).contains("\"srv-1\""));
        // overflow the buffer: one resync, then the newest changes
        for i in 0..5 {
            tx.send(Change::new("job", &format!("job-{i}"), Some("srv-1"), "updated")).unwrap();
        }
        assert!(text(&s.next().await.unwrap().unwrap()).contains("\"resync\""));
        assert!(text(&s.next().await.unwrap().unwrap()).contains("\"job-3\""));
        assert!(text(&s.next().await.unwrap().unwrap()).contains("\"job-4\""));
        shutdown.cancel();
        assert!(tokio::time::timeout(Duration::from_secs(1), s.next()).await.unwrap().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn keep_alive_while_idle() {
        let (_tx, rx) = broadcast::channel::<Change>(2);
        let (_ready_tx, ready_rx) = watch::channel(true);
        let mut s = Box::pin(frames(rx, ready_rx, CancellationToken::new()));
        assert_eq!(&s.next().await.unwrap().unwrap()[..], READY_FRAME);
        assert_eq!(&s.next().await.unwrap().unwrap()[..], sse::KEEP_ALIVE_FRAME);
    }

    #[tokio::test]
    async fn the_detector_runs_only_while_somebody_listens() {
        let store = Store::open_in_memory().await.unwrap();
        let hub = Hub::new(store.clone(), CancellationToken::new());
        assert!(!hub.is_running());
        let (mut rx, mut ready) = hub.subscribe();
        assert!(hub.is_running());
        ready.wait_for(|r| *r).await.unwrap();
        // a write without a nudge is found by the next poll
        let g = ferry_core::EnvGroup::new("shared");
        store.create_env_group(&g).await.unwrap();
        let c = tokio::time::timeout(POLL * 3, rx.recv()).await.unwrap().unwrap();
        assert_eq!(c, Change::new("env_group", &g.id, None, "created"));
        // a nudge compares at once
        store.set_env(&g.id, "A", "1").await.unwrap();
        hub.nudge();
        let c = tokio::time::timeout(POLL / 2, rx.recv()).await.unwrap().unwrap();
        assert_eq!(c, Change::new("env_group", &g.id, None, "updated"));
        drop(rx);
        hub.nudge();
        for _ in 0..100 {
            if !hub.is_running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!hub.is_running());
        // and starts again for the next subscriber
        let (_rx, _) = hub.subscribe();
        assert!(hub.is_running());
    }
}
