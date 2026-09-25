//! Deploy and job logs: one append-only JSON-lines file per deploy/job
//! (`logs/deploys/<id>.log`, `logs/jobs/<id>.log`) plus an in-memory buffer
//! that live followers read from while the deploy/job is running.
//!
//! Every open log has one writer task. Lines arrive through a [`LogSink`]
//! (so the builder, the Docker pull and the engine itself can all write), get
//! a sequence number in the in-memory buffer and are appended to the file.
//! Followers keep a cursor (sequence number) into the buffer and wait on a
//! `watch` channel for more, so they never see a gap or a duplicate no matter
//! when they subscribe. Once a log is finished, its buffer is dropped and
//! readers replay the (flushed) file instead.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use ferry_core::{LogLine, LogSink, LogStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, warn};

use crate::util::lock;

/// Lines kept in memory per running log. Beyond that the oldest lines are
/// only in the file; a follower that subscribes late gets a note instead.
const MAX_BUFFERED_LINES: usize = 100_000;
/// Lines written per batch before the file is flushed.
const MAX_BATCH: usize = 1024;
/// Lines still queued when a log is finished are written up to this many.
const MAX_DRAIN_AFTER_FINISH: usize = 1_000_000;
/// How long `finish` waits for the writer to flush.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);

/// Which family of logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LogKind {
    Deploy,
    Job,
}

impl LogKind {
    fn dir_name(self) -> &'static str {
        match self {
            LogKind::Deploy => "deploys",
            LogKind::Job => "jobs",
        }
    }
}

type Key = (LogKind, String);
type ActiveMap = Arc<StdMutex<HashMap<Key, LogHandle>>>;

/// Shared state of one running log.
struct Shared {
    buf: StdMutex<Buffer>,
    /// Bumped after every batch (and when the log finishes).
    version: watch::Sender<u64>,
}

#[derive(Default)]
struct Buffer {
    lines: VecDeque<LogLine>,
    /// Sequence number of `lines[0]`.
    base: u64,
    finished: bool,
}

impl Shared {
    fn new() -> Self {
        Shared { buf: StdMutex::new(Buffer::default()), version: watch::channel(0).0 }
    }

    /// Lines with a sequence number `>= *next` (advancing `next`), the number
    /// of lines that were requested but are no longer buffered, and whether
    /// the log is finished (in which case nothing will follow).
    fn read_from(&self, next: &mut u64) -> (Vec<LogLine>, u64, bool) {
        let buf = lock(&self.buf);
        let mut skipped = 0;
        if *next < buf.base {
            skipped = buf.base - *next;
            *next = buf.base;
        }
        let start = usize::try_from(*next - buf.base).unwrap_or(usize::MAX);
        let lines: Vec<LogLine> = buf.lines.iter().skip(start).cloned().collect();
        *next += lines.len() as u64;
        (lines, skipped, buf.finished)
    }

    fn push(&self, batch: &[LogLine]) {
        {
            let mut buf = lock(&self.buf);
            for line in batch {
                buf.lines.push_back(line.clone());
            }
            while buf.lines.len() > MAX_BUFFERED_LINES {
                buf.lines.pop_front();
                buf.base += 1;
            }
        }
        self.version.send_modify(|v| *v += batch.len() as u64);
    }

    fn mark_finished(&self) {
        lock(&self.buf).finished = true;
        self.version.send_modify(|v| *v += 1);
    }
}

/// Write handle of an open log. Cheap to clone; every clone writes to the
/// same log. Lines written after [`LogHandle::finish`] are dropped.
#[derive(Clone)]
pub(crate) struct LogHandle {
    sink: LogSink,
    ctl: mpsc::UnboundedSender<oneshot::Sender<()>>,
    shared: Arc<Shared>,
}

impl LogHandle {
    /// A sink for producers such as the builder or an image pull.
    pub(crate) fn sink(&self) -> &LogSink {
        &self.sink
    }

    /// A Ferry message (conventionally starting with `==> `).
    pub(crate) fn system(&self, text: impl Into<String>) {
        self.sink.system(text);
    }

    /// Flush everything written so far, close the log and end the streams of
    /// its followers. Idempotent.
    pub(crate) async fn finish(&self) {
        let (tx, rx) = oneshot::channel();
        if self.ctl.send(tx).is_err() {
            return;
        }
        if tokio::time::timeout(FINISH_TIMEOUT, rx).await.is_err() {
            warn!("timed out waiting for a log writer to flush");
        }
    }
}

/// All deploy/job logs of the server.
pub(crate) struct LogHub {
    root: PathBuf,
    active: ActiveMap,
}

impl LogHub {
    /// `root` is `config.logs_dir()`.
    pub(crate) fn new(root: PathBuf) -> Self {
        LogHub { root, active: Arc::new(StdMutex::new(HashMap::new())) }
    }

    /// Directory of one family of logs.
    pub(crate) fn dir(&self, kind: LogKind) -> PathBuf {
        self.root.join(kind.dir_name())
    }

    /// File of one log; `None` for ids that are not a safe file name.
    pub(crate) fn path(&self, kind: LogKind, id: &str) -> Option<PathBuf> {
        safe_id(id).then(|| self.dir(kind).join(format!("{id}.log")))
    }

    /// The handle of a running log, creating (and starting the writer of) the
    /// log if needed. Appends to an existing file.
    pub(crate) fn open(&self, kind: LogKind, id: &str) -> LogHandle {
        let key = (kind, id.to_string());
        let mut active = lock(&self.active);
        if let Some(handle) = active.get(&key) {
            return handle.clone();
        }
        let (lines_tx, lines_rx) = mpsc::unbounded_channel();
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared::new());
        let handle = LogHandle { sink: LogSink::new(lines_tx), ctl: ctl_tx, shared: shared.clone() };
        active.insert(key.clone(), handle.clone());
        drop(active);
        let path = self.path(kind, id);
        if path.is_none() {
            warn!(id, "log id is not a safe file name; keeping the log in memory only");
        }
        tokio::spawn(writer(path, key, shared, self.active.clone(), lines_rx, ctl_rx));
        handle
    }

    /// The handle of a log only if it is still open (never re-opens a
    /// finished log).
    pub(crate) fn get(&self, kind: LogKind, id: &str) -> Option<LogHandle> {
        lock(&self.active).get(&(kind, id.to_string())).cloned()
    }

    /// True while a log is open (its deploy/job is still running).
    pub(crate) fn is_open(&self, kind: LogKind, id: &str) -> bool {
        lock(&self.active).contains_key(&(kind, id.to_string()))
    }

    /// Replay a log. For a running log: with `follow`, keep streaming new
    /// lines until it finishes; without, the lines so far. For a finished
    /// log: the stored lines.
    pub(crate) fn stream(&self, kind: LogKind, id: &str, follow: bool) -> LogStream {
        let shared = lock(&self.active).get(&(kind, id.to_string())).map(|h| h.shared.clone());
        match shared {
            Some(shared) if follow => follow_stream(shared),
            Some(shared) => {
                let mut next = 0;
                let (lines, skipped, _) = shared.read_from(&mut next);
                let mut out = Vec::with_capacity(lines.len() + 1);
                if skipped > 0 {
                    out.push(skipped_note(skipped));
                }
                out.extend(lines);
                Box::pin(futures::stream::iter(out))
            }
            None => match self.path(kind, id) {
                Some(path) => file_stream(path),
                None => Box::pin(futures::stream::empty()),
            },
        }
    }

    /// Delete the file of a finished log (missing = Ok).
    pub(crate) async fn remove(&self, kind: LogKind, id: &str) {
        if self.is_open(kind, id) {
            return;
        }
        if let Some(path) = self.path(kind, id)
            && let Err(e) = tokio::fs::remove_file(&path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!(path = %path.display(), "could not delete log file: {e}");
        }
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn skipped_note(skipped: u64) -> LogLine {
    LogLine::system(format!(
        "==> {skipped} earlier line(s) are only available once this log is complete (the log is very long)"
    ))
}

fn follow_stream(shared: Arc<Shared>) -> LogStream {
    // Subscribe before the first read so no update between the read and the
    // wait can be missed.
    let mut changes = shared.version.subscribe();
    Box::pin(async_stream::stream! {
        let mut next = 0u64;
        loop {
            let (lines, skipped, finished) = shared.read_from(&mut next);
            if skipped > 0 {
                yield skipped_note(skipped);
            }
            for line in lines {
                yield line;
            }
            if finished || changes.changed().await.is_err() {
                break;
            }
        }
    })
}

fn file_stream(path: PathBuf) -> LogStream {
    Box::pin(async_stream::stream! {
        let file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(path = %path.display(), "cannot read log file: {e}");
                }
                return;
            }
        };
        let mut lines = BufReader::new(file).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(text)) => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<LogLine>(&text) {
                        Ok(line) => yield line,
                        Err(e) => debug!(path = %path.display(), "skipping malformed log line: {e}"),
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(path = %path.display(), "error reading log file: {e}");
                    break;
                }
            }
        }
    })
}

async fn open_file(path: &Path) -> Option<BufWriter<tokio::fs::File>> {
    if let Some(dir) = path.parent()
        && let Err(e) = tokio::fs::create_dir_all(dir).await
    {
        warn!(dir = %dir.display(), "cannot create log directory: {e}");
        return None;
    }
    match tokio::fs::OpenOptions::new().create(true).append(true).open(path).await {
        Ok(f) => Some(BufWriter::new(f)),
        Err(e) => {
            warn!(path = %path.display(), "cannot open log file (the log is kept in memory only): {e}");
            None
        }
    }
}

async fn write_batch(file: &mut Option<BufWriter<tokio::fs::File>>, path: Option<&Path>, batch: &[LogLine]) {
    let Some(f) = file.as_mut() else { return };
    let mut bytes = Vec::with_capacity(batch.len() * 96);
    for line in batch {
        if serde_json::to_writer(&mut bytes, line).is_ok() {
            bytes.push(b'\n');
        }
    }
    let res = async {
        f.write_all(&bytes).await?;
        f.flush().await
    }
    .await;
    if let Err(e) = res {
        let shown = path.map(|p| p.display().to_string()).unwrap_or_default();
        warn!(path = %shown, "writing log file failed (the log is kept in memory only): {e}");
        *file = None;
    }
}

/// The single writer of one log.
async fn writer(
    path: Option<PathBuf>,
    key: Key,
    shared: Arc<Shared>,
    active: ActiveMap,
    mut lines: mpsc::UnboundedReceiver<LogLine>,
    mut ctl: mpsc::UnboundedReceiver<oneshot::Sender<()>>,
) {
    let mut file = match &path {
        Some(p) => open_file(p).await,
        None => None,
    };
    let mut acks = Vec::new();
    let mut batch = Vec::new();
    loop {
        tokio::select! {
            // `finish` first, so a chatty producer cannot starve it.
            biased;
            ack = ctl.recv() => {
                if let Some(ack) = ack {
                    acks.push(ack);
                }
                break;
            }
            line = lines.recv() => {
                // The map keeps a sender alive until the log is finished.
                let Some(line) = line else { break };
                batch.push(line);
                while batch.len() < MAX_BATCH {
                    match lines.try_recv() {
                        Ok(l) => batch.push(l),
                        Err(_) => break,
                    }
                }
                shared.push(&batch);
                write_batch(&mut file, path.as_deref(), &batch).await;
                batch.clear();
            }
        }
    }
    // Everything sent before `finish` is already queued: drain it (bounded,
    // in case a producer keeps writing).
    let mut drained = 0usize;
    while drained < MAX_DRAIN_AFTER_FINISH
        && let Ok(l) = lines.try_recv()
    {
        drained += 1;
        batch.push(l);
        if batch.len() >= MAX_BATCH {
            shared.push(&batch);
            write_batch(&mut file, path.as_deref(), &batch).await;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        shared.push(&batch);
        write_batch(&mut file, path.as_deref(), &batch).await;
    }
    if let Some(mut f) = file {
        let _ = f.shutdown().await;
    }
    // The file is complete: new readers may now use it instead of memory.
    {
        let mut map = lock(&active);
        if map.get(&key).is_some_and(|h| Arc::ptr_eq(&h.shared, &shared)) {
            map.remove(&key);
        }
    }
    shared.mark_finished();
    while let Ok(ack) = ctl.try_recv() {
        acks.push(ack);
    }
    for ack in acks {
        let _ = ack.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::LogStreamKind;
    use futures::StreamExt;

    fn hub() -> (tempfile::TempDir, LogHub) {
        let dir = tempfile::tempdir().unwrap();
        let hub = LogHub::new(dir.path().join("logs"));
        (dir, hub)
    }

    fn texts(lines: &[LogLine]) -> Vec<String> {
        lines.iter().map(|l| l.line.clone()).collect()
    }

    #[tokio::test]
    async fn writes_file_and_replays_after_finish() {
        let (_dir, hub) = hub();
        let h = hub.open(LogKind::Deploy, "dep-1");
        h.system("==> one");
        h.sink().stderr("two");
        h.sink().send(LogLine::new(LogStreamKind::Stdout, "three").with_instance("abc123"));
        h.finish().await;
        assert!(!hub.is_open(LogKind::Deploy, "dep-1"));
        let lines: Vec<LogLine> = hub.stream(LogKind::Deploy, "dep-1", true).collect().await;
        assert_eq!(texts(&lines), ["==> one", "two", "three"]);
        assert_eq!(lines[1].stream, LogStreamKind::Stderr);
        assert_eq!(lines[2].instance.as_deref(), Some("abc123"));
        // File format: one JSON LogLine per line.
        let raw = std::fs::read_to_string(hub.path(LogKind::Deploy, "dep-1").unwrap()).unwrap();
        assert_eq!(raw.lines().count(), 3);
        let first: LogLine = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert_eq!(first.line, "==> one");
        // Writes after finish are dropped; finish is idempotent.
        h.system("late");
        h.finish().await;
        let again: Vec<LogLine> = hub.stream(LogKind::Deploy, "dep-1", false).collect().await;
        assert_eq!(again.len(), 3);
    }

    #[tokio::test]
    async fn follow_sees_past_and_future_lines_without_gaps_or_duplicates() {
        let (_dir, hub) = hub();
        let h = hub.open(LogKind::Job, "job-1");
        for i in 0..50 {
            h.system(format!("early {i}"));
        }
        let early = hub.stream(LogKind::Job, "job-1", true);
        let reader = tokio::spawn(early.collect::<Vec<_>>());
        // A late subscriber joins halfway.
        for i in 0..50 {
            h.system(format!("late {i}"));
            tokio::task::yield_now().await;
        }
        let late = tokio::spawn(hub.stream(LogKind::Job, "job-1", true).collect::<Vec<_>>());
        for i in 50..100 {
            h.system(format!("late {i}"));
        }
        h.finish().await;
        let expected: Vec<String> =
            (0..50).map(|i| format!("early {i}")).chain((0..100).map(|i| format!("late {i}"))).collect();
        let a = tokio::time::timeout(Duration::from_secs(5), reader).await.unwrap().unwrap();
        let b = tokio::time::timeout(Duration::from_secs(5), late).await.unwrap().unwrap();
        assert_eq!(texts(&a), expected);
        assert_eq!(texts(&b), expected);
    }

    #[tokio::test]
    async fn snapshot_of_running_log_and_empty_for_unknown() {
        let (_dir, hub) = hub();
        let h = hub.open(LogKind::Deploy, "dep-2");
        h.system("a");
        h.system("b");
        // Wait until the writer has taken them.
        for _ in 0..100 {
            let n = hub.stream(LogKind::Deploy, "dep-2", false).count().await;
            if n == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let snap: Vec<LogLine> = hub.stream(LogKind::Deploy, "dep-2", false).collect().await;
        assert_eq!(texts(&snap), ["a", "b"]);
        assert!(hub.is_open(LogKind::Deploy, "dep-2"));
        h.finish().await;
        assert_eq!(hub.stream(LogKind::Deploy, "nope", true).count().await, 0);
        assert_eq!(hub.stream(LogKind::Deploy, "../etc/passwd", false).count().await, 0);
        assert!(hub.path(LogKind::Deploy, "a/b").is_none());
    }

    #[tokio::test]
    async fn open_returns_the_running_log_and_reopen_appends() {
        let (_dir, hub) = hub();
        let a = hub.open(LogKind::Deploy, "dep-3");
        let b = hub.open(LogKind::Deploy, "dep-3");
        a.system("from a");
        b.system("from b");
        b.finish().await;
        let c = hub.open(LogKind::Deploy, "dep-3");
        c.system("appended");
        c.finish().await;
        let lines: Vec<LogLine> = hub.stream(LogKind::Deploy, "dep-3", false).collect().await;
        assert_eq!(texts(&lines), ["from a", "from b", "appended"]);
        hub.remove(LogKind::Deploy, "dep-3").await;
        assert_eq!(hub.stream(LogKind::Deploy, "dep-3", false).count().await, 0);
    }

    #[tokio::test]
    async fn many_concurrent_followers_all_get_everything() {
        let (_dir, hub) = hub();
        let h = hub.open(LogKind::Deploy, "dep-4");
        let mut readers = Vec::new();
        let writer = {
            let h = h.clone();
            tokio::spawn(async move {
                for i in 0..2000 {
                    h.sink().stdout(format!("{i}"));
                    if i % 100 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
            })
        };
        for _ in 0..8 {
            readers.push(tokio::spawn(hub.stream(LogKind::Deploy, "dep-4", true).collect::<Vec<_>>()));
            tokio::task::yield_now().await;
        }
        writer.await.unwrap();
        h.finish().await;
        let expected: Vec<String> = (0..2000).map(|i| i.to_string()).collect();
        for r in readers {
            let lines = tokio::time::timeout(Duration::from_secs(5), r).await.unwrap().unwrap();
            assert_eq!(texts(&lines), expected);
        }
    }

    #[test]
    fn buffer_cap_reports_skipped_lines() {
        let shared = Shared::new();
        let batch: Vec<LogLine> = (0..(MAX_BUFFERED_LINES + 10)).map(|i| LogLine::system(i.to_string())).collect();
        shared.push(&batch);
        let mut next = 0;
        let (lines, skipped, finished) = shared.read_from(&mut next);
        assert_eq!(skipped, 10);
        assert_eq!(lines.len(), MAX_BUFFERED_LINES);
        assert_eq!(lines[0].line, "10");
        assert!(!finished);
        assert_eq!(next, (MAX_BUFFERED_LINES + 10) as u64);
    }
}
