//! Child-process helpers: capture or stream output, honor cancellation and
//! timeouts, and never leave stray processes behind.
//!
//! Every child is started in its own process group (Unix) so that a cancel
//! can signal the whole tree (`docker` → `docker-buildx`, `git` → `ssh` /
//! `git-remote-https`), not just the direct child.
//!
//! Children never outlive the server: dropping their owner (e.g. the runtime
//! shutting down with builds in flight) kills their group, groups still
//! running when the process calls `exit` (which skips destructors, e.g. a
//! forced shutdown) are killed by an `atexit` hook, and on Linux the direct
//! child also gets `SIGKILL` if the server dies abruptly (`kill -9`). Within
//! [`record_children_in`], running children are also recorded on disk so
//! that the next server can reap what a `kill -9` left ([`crate::orphans`]).

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferry_core::CancellationToken;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};

/// Longest line forwarded to a log sink; longer lines are truncated.
const MAX_LINE_BYTES: usize = 16 * 1024;
/// How long a canceled child gets to exit after SIGTERM before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(5);
/// How long we wait for the output pipes to close after SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(5);

/// Why a child process could not run to completion.
#[derive(Debug)]
pub(crate) enum RunError {
    /// The program could not be started (not installed, permissions...).
    Spawn(io::Error),
    /// Waiting for the child failed.
    Io(io::Error),
    Canceled,
    TimedOut(Duration),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Spawn(e) => write!(f, "could not start: {e}"),
            RunError::Io(e) => write!(f, "{e}"),
            RunError::Canceled => f.write_str("canceled"),
            RunError::TimedOut(d) => write!(f, "timed out after {}s", d.as_secs()),
        }
    }
}

/// Prepare a command for supervised execution: no stdin, own process group,
/// killed if the owning future is dropped (or the server dies, see the
/// module docs).
pub(crate) fn supervise(cmd: &mut Command) {
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(target_os = "linux")]
    {
        let parent = std::process::id();
        // SAFETY: runs in the forked child before exec and only calls
        // async-signal-safe functions (prctl, getppid). The death signal is
        // tied to the spawning *thread*: children are spawned from async
        // code, i.e. runtime worker threads, which live as long as the
        // runtime (whose shutdown kills the children anyway).
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // The parent may have died before prctl took effect.
                if u32::try_from(libc::getppid()).ok() != Some(parent) {
                    return Err(io::Error::other("the parent process exited"));
                }
                Ok(())
            });
        }
    }
}

/// Process groups of running children, killed by an `atexit` hook if the
/// process exits without dropping their owners (`std::process::exit`).
#[cfg(unix)]
mod live_groups {
    use std::sync::{Mutex, Once, TryLockError};

    static GROUPS: Mutex<Vec<i32>> = Mutex::new(Vec::new());
    static HOOK: Once = Once::new();

    extern "C" fn kill_all_at_exit() {
        // Never block process exit on a lock held by another thread.
        let groups = match GROUPS.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        for pgid in groups.iter() {
            // SAFETY: kill(2) has no memory-safety preconditions; a negative
            // pid targets a process group we created.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }

    pub(super) fn add(pgid: i32) {
        HOOK.call_once(|| {
            // SAFETY: registers a function without preconditions; it only
            // uses a static and async-signal-safe calls.
            let rc = unsafe { libc::atexit(kill_all_at_exit) };
            if rc != 0 {
                tracing::warn!("cannot register the child cleanup exit hook");
            }
        });
        GROUPS.lock().unwrap_or_else(|p| p.into_inner()).push(pgid);
    }

    pub(super) fn remove(pgid: i32) {
        let mut groups = GROUPS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(i) = groups.iter().position(|p| *p == pgid) {
            groups.swap_remove(i);
        }
    }
}

tokio::task_local! {
    /// Where the running build records its children (see [`record_children_in`]).
    static CHILD_RECORDS: PathBuf;
}

/// Run `fut`, recording every child it spawns in `dir` (created on demand)
/// for as long as the child runs, so a server killed with SIGKILL leaves a
/// trace of them for [`crate::orphans::reap`].
pub(crate) async fn record_children_in<F: Future>(dir: PathBuf, fut: F) -> F::Output {
    CHILD_RECORDS.scope(dir, fut).await
}

fn record_child(pid: Option<u32>, program: &std::ffi::OsStr) -> Option<PathBuf> {
    let pid = pid?;
    let dir = CHILD_RECORDS.try_with(Clone::clone).ok()?;
    match crate::orphans::write_record(&dir, pid, program) {
        Ok(path) => Some(path),
        Err(e) => {
            tracing::debug!(pid, dir = %dir.display(), "cannot record a build child: {e}");
            None
        }
    }
}

/// Cap on captured stdout/stderr of [`run_capture`] (git output is small).
const CAPTURE_LIMIT: u64 = 8 * 1024 * 1024;

/// Read `r` to the end, keeping at most `limit` bytes (the rest is drained so
/// the writer never blocks).
async fn read_capped<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>, limit: u64) {
    let _ = (&mut *r).take(limit).read_to_end(buf).await;
    let _ = tokio::io::copy(r, &mut tokio::io::sink()).await;
}

fn spawn(cmd: &mut Command) -> Result<(Child, GroupGuard), RunError> {
    supervise(cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(RunError::Spawn)?;
    let record = record_child(child.id(), cmd.as_std().get_program());
    let guard = GroupGuard::new(child.id(), record);
    Ok((child, guard))
}

/// Run `cmd` to completion and capture its output.
pub(crate) async fn run_capture(
    mut cmd: Command,
    cancel: Option<&CancellationToken>,
    timeout: Option<Duration>,
) -> Result<Output, RunError> {
    let (mut child, guard) = spawn(&mut cmd)?;
    let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
        // Invariant: both pipes were requested in `spawn`.
        return Err(RunError::Io(io::Error::other("child pipes unavailable")));
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let status = {
        let pumps = async {
            tokio::join!(
                read_capped(&mut out, &mut stdout, CAPTURE_LIMIT),
                read_capped(&mut err, &mut stderr, CAPTURE_LIMIT)
            );
        };
        drive(&mut child, guard.pid(), pumps, cancel, timeout, Some(PIPE_GRACE)).await
    };
    guard.disarm();
    Ok(Output { status: status?, stdout, stderr })
}

/// Run `cmd`, calling `on_stdout` / `on_stderr` for every output line as it
/// arrives. Returns the exit status and the last `tail_len` lines (both
/// streams interleaved in arrival order).
pub(crate) async fn run_streaming<F, G>(
    mut cmd: Command,
    cancel: &CancellationToken,
    tail_len: usize,
    on_stdout: F,
    on_stderr: G,
) -> Result<(ExitStatus, Vec<String>), RunError>
where
    F: FnMut(&str) + Send,
    G: FnMut(&str) + Send,
{
    let (mut child, guard) = spawn(&mut cmd)?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        // Invariant: both pipes were requested in `spawn`.
        return Err(RunError::Io(io::Error::other("child pipes unavailable")));
    };

    let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::with_capacity(tail_len)));
    let push_tail = |line: &str| {
        if tail_len == 0 {
            return;
        }
        if let Ok(mut t) = tail.lock() {
            if t.len() == tail_len {
                t.pop_front();
            }
            t.push_back(line.to_string());
        }
    };
    let mut on_stdout = on_stdout;
    let mut on_stderr = on_stderr;
    let status = {
        let out_pump = pump_lines(stdout, |l| {
            push_tail(l);
            on_stdout(l);
        });
        let err_pump = pump_lines(stderr, |l| {
            push_tail(l);
            on_stderr(l);
        });
        let pumps = async {
            tokio::join!(out_pump, err_pump);
        };
        drive(&mut child, guard.pid(), pumps, Some(cancel), None, Some(PIPE_GRACE)).await
    };
    guard.disarm();
    let status = status?;
    let lines = tail.lock().map(|t| t.iter().cloned().collect()).unwrap_or_default();
    Ok((status, lines))
}

/// Run `cmd`, handing its stdout to `consume` (e.g. a blocking tar extractor
/// in `spawn_blocking`) while capturing stderr. Returns the exit status, the
/// captured stderr and the consumer's result. The consumer must read until
/// EOF or drop the pipe, otherwise the child may block forever.
pub(crate) async fn run_with_stdout<T, C, Fut>(
    mut cmd: Command,
    cancel: &CancellationToken,
    consume: C,
) -> Result<(ExitStatus, Vec<u8>, T), RunError>
where
    C: FnOnce(ChildStdout) -> Fut,
    Fut: Future<Output = T>,
{
    let (mut child, guard) = spawn(&mut cmd)?;
    let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        // Invariant: both pipes were requested in `spawn`.
        return Err(RunError::Io(io::Error::other("child pipes unavailable")));
    };
    let mut err_buf = Vec::new();
    let mut consumed: Option<T> = None;
    let status = {
        let pumps = async {
            let (value, _) = tokio::join!(consume(stdout), read_capped(&mut stderr, &mut err_buf, 1024 * 1024));
            consumed = Some(value);
        };
        // The consumer may still be busy after EOF (e.g. finishing an extraction).
        drive(&mut child, guard.pid(), pumps, Some(cancel), None, None).await
    };
    guard.disarm();
    let status = status?;
    match consumed {
        Some(value) => Ok((status, err_buf, value)),
        None => Err(RunError::Io(io::Error::other("output of the child process was not fully read"))),
    }
}

/// How long output pipes may stay open after the child exited normally
/// (a leftover background process holding them gets killed after that).
const PIPE_GRACE: Duration = Duration::from_secs(10);

/// Supervise a spawned child while `pumps` drains its output.
///
/// * Normal exit: return the status once `pumps` completes — waiting at most
///   `pipe_grace` (then the process group is killed so a leftover background
///   process holding the pipes cannot hang us), or indefinitely with `None`.
/// * Cancel / timeout: SIGTERM the process group; as soon as the child has
///   exited (or after [`TERM_GRACE`]) SIGKILL whatever is left of the group
///   (e.g. a grandchild that was being forked when SIGTERM arrived), then
///   return the reason.
async fn drive<P>(
    child: &mut Child,
    pid: Option<u32>,
    pumps: P,
    cancel: Option<&CancellationToken>,
    timeout: Option<Duration>,
    pipe_grace: Option<Duration>,
) -> Result<ExitStatus, RunError>
where
    P: Future<Output = ()>,
{
    tokio::pin!(pumps);
    let mut pumps_done = false;
    let never = CancellationToken::new();
    let cancel = cancel.unwrap_or(&never);
    let deadline = async {
        match timeout {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline);

    let stop = loop {
        tokio::select! {
            _ = &mut pumps, if !pumps_done => pumps_done = true,
            status = child.wait() => {
                let status = status.map_err(RunError::Io)?;
                if !pumps_done {
                    match pipe_grace {
                        None => (&mut pumps).await,
                        Some(grace) => {
                            if tokio::time::timeout(grace, &mut pumps).await.is_err() {
                                tracing::debug!(?pid, "child exited but its output pipes stay open; killing its group");
                                signal_group(pid, Signal::Kill);
                                let _ = tokio::time::timeout(KILL_GRACE, &mut pumps).await;
                            }
                        }
                    }
                }
                return Ok(status);
            }
            _ = cancel.cancelled() => break RunError::Canceled,
            _ = &mut deadline => break RunError::TimedOut(timeout.unwrap_or_default()),
        }
    };

    signal_group(pid, Signal::Term);
    let exited = tokio::time::timeout(TERM_GRACE, async {
        loop {
            tokio::select! {
                _ = &mut pumps, if !pumps_done => pumps_done = true,
                _ = child.wait() => break,
            }
        }
    })
    .await
    .is_ok();
    // Whatever survived SIGTERM belongs to the canceled work.
    signal_group(pid, Signal::Kill);
    if !exited && tokio::time::timeout(KILL_GRACE, child.wait()).await.is_err() {
        tracing::warn!(?pid, "child process did not exit after SIGKILL; abandoning it");
    }
    if !pumps_done {
        let _ = tokio::time::timeout(KILL_GRACE, &mut pumps).await;
    }
    Err(stop)
}

#[derive(Debug, Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

#[cfg(unix)]
fn signal_group(pid: Option<u32>, sig: Signal) {
    let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 0) else {
        return;
    };
    let signo = match sig {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: kill(2) has no memory-safety preconditions. A negative pid
    // targets the process group we created with `process_group(0)`, whose id
    // equals the (not yet reaped) child's pid.
    unsafe {
        libc::kill(-pid, signo);
    }
}

#[cfg(not(unix))]
fn signal_group(_pid: Option<u32>, _sig: Signal) {
    // Without process groups we rely on `kill_on_drop` of the direct child.
}

/// Kills the child's whole process group if the owning future is dropped
/// before the child was reaped (e.g. the build task was aborted), and
/// removes the child's record (see [`record_children_in`]).
struct GroupGuard {
    pid: Option<u32>,
    armed: bool,
    record: Option<PathBuf>,
}

impl GroupGuard {
    fn new(pid: Option<u32>, record: Option<PathBuf>) -> Self {
        #[cfg(unix)]
        if let Some(pgid) = pid.and_then(|p| i32::try_from(p).ok()) {
            live_groups::add(pgid);
        }
        GroupGuard { pid, armed: true, record }
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The child has been reaped: its pid may be reused, never signal it again.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            signal_group(self.pid, Signal::Kill);
        }
        if let Some(record) = &self.record {
            let _ = std::fs::remove_file(record);
        }
        #[cfg(unix)]
        if let Some(pgid) = self.pid.and_then(|p| i32::try_from(p).ok()) {
            live_groups::remove(pgid);
        }
    }
}

/// Read `r` line by line (lossy UTF-8, `\r` progress updates collapsed to
/// their final state, over-long lines truncated) and call `f` for each line.
pub(crate) async fn pump_lines<R, F>(r: R, mut f: F)
where
    R: AsyncRead + Unpin,
    F: FnMut(&str),
{
    let mut reader = BufReader::with_capacity(16 * 1024, r);
    let mut line: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        let buf = match reader.fill_buf().await {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("error reading child output: {e}");
                break;
            }
        };
        if buf.is_empty() {
            break;
        }
        let (chunk, found_newline) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], true),
            None => (buf, false),
        };
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        if chunk.len() > room {
            truncated = true;
        }
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let consumed = chunk.len() + usize::from(found_newline);
        reader.consume(consumed);
        if found_newline {
            f(&finish_line(&line, truncated));
            line.clear();
            truncated = false;
        }
    }
    if !line.is_empty() {
        f(&finish_line(&line, truncated));
    }
}

fn finish_line(raw: &[u8], truncated: bool) -> String {
    let text = String::from_utf8_lossy(raw);
    // A carriage return means "redraw this line": keep the last non-empty state.
    let text = text.split('\r').rev().find(|s| !s.trim().is_empty()).unwrap_or("");
    let mut s = strip_ansi(text).trim_end().to_string();
    if truncated {
        s.push_str(" …[truncated]");
    }
    s
}

/// Remove terminal escape sequences (colors, cursor movement) and other
/// control characters except tabs, so logs render cleanly anywhere.
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: parameters, then one final byte in '@'..='~'.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: until BEL or ESC '\\'.
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pump_splits_lines_and_handles_cr() {
        let data: &[u8] = b"one\r\ntwo\nprogress 10%\rprogress 100%\n\xffbad utf8\nlast";
        let mut lines = Vec::new();
        pump_lines(data, |l| lines.push(l.to_string())).await;
        assert_eq!(lines, vec!["one", "two", "progress 100%", "\u{fffd}bad utf8", "last"]);
    }

    #[test]
    fn strips_terminal_escapes() {
        assert_eq!(strip_ansi("\u{1b}[33m1 warning found\u{1b}[0m"), "1 warning found");
        assert_eq!(strip_ansi("a\u{1b}]0;title\u{7}b\tc\u{8}"), "ab\tc");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[tokio::test]
    async fn pump_truncates_long_lines() {
        let long = vec![b'a'; MAX_LINE_BYTES * 3];
        let mut data = long.clone();
        data.extend_from_slice(b"\nshort\n");
        let mut lines = Vec::new();
        pump_lines(&data[..], |l| lines.push(l.to_string())).await;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("[truncated]"));
        assert!(lines[0].len() < MAX_LINE_BYTES + 32);
        assert_eq!(lines[1], "short");
    }

    #[tokio::test]
    async fn capture_and_status() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err >&2; exit 3"]);
        let out = run_capture(cmd, None, None).await.unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
    }

    #[tokio::test]
    async fn streaming_collects_tail() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "for i in 1 2 3 4 5; do echo line$i; done; echo oops >&2"]);
        let cancel = CancellationToken::new();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (status, tail) =
            run_streaming(cmd, &cancel, 3, |l| out.push(l.to_string()), |l| err.push(l.to_string())).await.unwrap();
        assert!(status.success());
        assert_eq!(out.len(), 5);
        assert_eq!(err, vec!["oops"]);
        // stdout and stderr are read concurrently: only the size is deterministic.
        assert_eq!(tail.len(), 3);
        assert!(tail.iter().all(|l| l == "oops" || l.starts_with("line")), "{tail:?}");
    }

    #[tokio::test]
    async fn cancel_kills_process_tree() {
        let mut cmd = Command::new("sh");
        // A grandchild keeps the pipes open: only a group kill ends it.
        cmd.args(["-c", "sleep 30 & sleep 30; wait"]);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let started = std::time::Instant::now();
        let res = run_streaming(cmd, &cancel, 0, |_| {}, |_| {}).await;
        assert!(matches!(res, Err(RunError::Canceled)), "{res:?}");
        assert!(started.elapsed() < Duration::from_secs(8), "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn cancel_kills_stragglers_as_soon_as_the_child_exits() {
        let mut cmd = Command::new("sh");
        // The background job ignores SIGTERM and keeps our pipes open.
        cmd.args(["-c", "(trap '' TERM; exec sleep 30) & echo started; wait"]);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let started = std::time::Instant::now();
        let res = run_streaming(
            cmd,
            &cancel,
            0,
            move |l| {
                if l == "started" {
                    c2.cancel();
                }
            },
            |_| {},
        )
        .await;
        assert!(matches!(res, Err(RunError::Canceled)), "{res:?}");
        assert!(started.elapsed() < TERM_GRACE, "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn sigkill_after_grace_when_sigterm_is_ignored() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "trap '' TERM; echo started; sleep 30"]);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let started = std::time::Instant::now();
        let res = run_streaming(
            cmd,
            &cancel,
            0,
            move |l| {
                if l == "started" {
                    c2.cancel();
                }
            },
            |_| {},
        )
        .await;
        assert!(matches!(res, Err(RunError::Canceled)), "{res:?}");
        let took = started.elapsed();
        assert!(took >= TERM_GRACE && took < TERM_GRACE + Duration::from_secs(5), "took {took:?}");
    }

    #[tokio::test]
    async fn exited_child_with_lingering_pipe_holder() {
        // Normal exit while a background process keeps stdout open: we wait
        // PIPE_GRACE at most, then kill it — never hang.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & echo done"]);
        let started = std::time::Instant::now();
        let out = run_capture(cmd, None, None).await.unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
        assert!(started.elapsed() < PIPE_GRACE + Duration::from_secs(5), "took {:?}", started.elapsed());
    }

    /// Wait until `pid` (a direct child of this process) has exited.
    #[cfg(unix)]
    fn child_exits_within(pid: i32, limit: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < limit {
            let mut status = 0;
            // SAFETY: plain syscall on a pid we spawned.
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if r == pid || r == -1 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[cfg(unix)]
    fn wait_for_pid_file(path: &std::path::Path) -> i32 {
        let start = std::time::Instant::now();
        loop {
            if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()) {
                return pid;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "child never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// ferryd's graceful shutdown drops the runtime with builds in flight:
    /// their `docker build` must not survive as orphans.
    #[cfg(unix)]
    #[test]
    fn runtime_shutdown_kills_running_children() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let script = format!("echo $$ > '{}.tmp' && mv '{0}.tmp' '{0}' && exec sleep 30", pid_file.display());
        rt.spawn(async move {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", &script]);
            let _ = run_streaming(cmd, &CancellationToken::new(), 0, |_| {}, |_| {}).await;
        });
        let pid = wait_for_pid_file(&pid_file);
        drop(rt);
        assert!(child_exits_within(pid, Duration::from_secs(5)), "child {pid} survived the runtime");
    }

    /// Runs only inside [`process_exit_kills_running_children`]'s child
    /// process: start a supervised child, then leave with
    /// `std::process::exit` (what ferryd does on a second signal), which
    /// skips every destructor.
    #[cfg(unix)]
    #[test]
    fn exit_helper() {
        let Some(pid_file) = std::env::var_os("FERRY_BUILD_EXIT_HELPER") else { return };
        let pid_file = std::path::PathBuf::from(pid_file);
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        rt.block_on(async move {
            let script = format!("echo $$ > '{}.tmp' && mv '{0}.tmp' '{0}' && exec sleep 30", pid_file.display());
            tokio::spawn(async move {
                let mut cmd = Command::new("sh");
                cmd.args(["-c", &script]);
                let _ = run_streaming(cmd, &CancellationToken::new(), 0, |_| {}, |_| {}).await;
            });
            while !pid_file.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            std::process::exit(0);
        });
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_kills_running_children() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process::tests::exit_helper", "--nocapture", "--test-threads=1"])
            .env("FERRY_BUILD_EXIT_HELPER", &pid_file)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{status}");
        let pid = wait_for_pid_file(&pid_file);
        // The child was re-parented to init: wait until it is gone.
        let start = std::time::Instant::now();
        // SAFETY: signal 0 only checks that the pid exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(start.elapsed() < Duration::from_secs(5), "child {pid} outlived its parent");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[tokio::test]
    async fn children_are_recorded_while_they_run() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("children");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &format!("echo $$; cat '{}'/$$", dir.display())]);
        let out = record_children_in(dir.clone(), run_capture(cmd, None, None)).await.unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut lines = stdout.lines();
        let pid = lines.next().unwrap();
        let record = crate::orphans::parse_record(lines.next().unwrap()).unwrap();
        assert_eq!((record.pid.to_string().as_str(), record.program.as_str()), (pid, "sh"));
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "the record outlived the child");

        // Outside a build nothing is recorded.
        let mut cmd = Command::new("true");
        cmd.arg("x");
        run_capture(cmd, None, None).await.unwrap();
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn timeout_is_reported() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let res = run_capture(cmd, None, Some(Duration::from_millis(200))).await;
        assert!(matches!(res, Err(RunError::TimedOut(_))), "{res:?}");
    }

    #[tokio::test]
    async fn missing_program_is_spawn_error() {
        let cmd = Command::new("definitely-not-a-real-program-ferry");
        assert!(matches!(run_capture(cmd, None, None).await, Err(RunError::Spawn(_))));
    }
}
