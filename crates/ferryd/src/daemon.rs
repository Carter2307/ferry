//! Running the server in the background.
//!
//! `ferryd start` re-executes the binary as a detached `ferryd run` whose
//! output goes to `<data-dir>/ferryd.log`, waits until that server listens
//! and gives the terminal back. `stop`, `status` and `logs` find the server
//! again through its data directory: the lock it holds (`ferryd.lock`) says
//! whether it is alive, and `ferryd.json` says what it is (pid, addresses).

use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The exit code of `ferryd status` when no server runs (as `systemctl
/// status` and LSB init scripts do).
const NOT_RUNNING: u8 = 3;
/// `ferryd start` gives up waiting for the server to listen after this long
/// (the server itself keeps starting).
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// A graceful shutdown drains connections, stops jobs and records
/// interrupted deploys: the sum of the server's own deadlines, and a margin.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
/// `ferryd start` moves a log bigger than this to `ferryd.log.1`.
const LOG_ROTATE_BYTES: u64 = 10 << 20;
/// Where the API listens by default: what `ferry login` uses when no server is named.
const DEFAULT_API_URL: &str = "http://127.0.0.1:7878";
/// `ferryd logs` reads at most this much of the end of the log.
const LOG_TAIL_BYTES: u64 = 4 << 20;

/// What a running server wrote about itself in `<data-dir>/ferryd.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerState {
    pub pid: u32,
    pub version: String,
    pub started_at: DateTime<Utc>,
    /// Started by `ferryd start`: its output is in `ferryd.log`.
    pub background: bool,
    /// Set once the server listens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<Ready>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ready {
    /// Where the dashboard and the API answer.
    pub api_url: String,
    /// The lines of the startup banner under its title.
    pub summary: Vec<String>,
}

pub fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ferryd.lock")
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ferryd.json")
}

pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ferryd.log")
}

/// `ferryd.json` for the lifetime of a server: written when it has the data
/// directory's lock, completed when it listens, removed when it stops.
pub struct StateFile {
    path: PathBuf,
    state: ServerState,
}

impl StateFile {
    pub fn create(data_dir: &Path, background: bool) -> anyhow::Result<Self> {
        let file = Self {
            path: state_path(data_dir),
            state: ServerState {
                pid: std::process::id(),
                version: ferry_core::VERSION.to_string(),
                started_at: Utc::now(),
                background,
                ready: None,
            },
        };
        file.write()?;
        Ok(file)
    }

    pub fn set_ready(&mut self, ready: Ready) -> anyhow::Result<()> {
        self.state.ready = Some(ready);
        self.write()
    }

    /// Replace the file in one step: a reader never sees half of it.
    fn write(&self) -> anyhow::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&self.state).context("encoding the server state")?;
        let _ = std::fs::remove_file(&tmp);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(json.as_bytes()).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("writing {}", self.path.display()))
    }
}

impl Drop for StateFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The server that holds a data directory.
#[derive(Debug, PartialEq)]
pub struct Running {
    /// `None` when it left no `ferryd.json` (a version that wrote none).
    pub state: Option<ServerState>,
}

/// Does a server hold the lock of this data directory?
fn is_locked(data_dir: &Path) -> anyhow::Result<bool> {
    let path = lock_path(data_dir);
    let file = match std::fs::OpenOptions::new().write(true).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => Ok(false), // released when `file` is dropped
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {}", path.display())),
    }
}

fn read_state(data_dir: &Path) -> Option<ServerState> {
    serde_json::from_str(&std::fs::read_to_string(state_path(data_dir)).ok()?).ok()
}

/// The server running with this data directory, if any. `ferryd.json` only
/// counts while the lock is held: a server that was killed leaves one behind.
pub fn running(data_dir: &Path) -> anyhow::Result<Option<Running>> {
    if !is_locked(data_dir)? {
        return Ok(None);
    }
    Ok(Some(Running { state: read_state(data_dir) }))
}

/// ` --data-dir <dir>` to append to the commands printed as hints, when the
/// data directory isn't the one `ferryd` uses by default.
pub fn data_dir_hint(data_dir: &Path, is_default: bool) -> String {
    if is_default {
        return String::new();
    }
    let dir = data_dir.display().to_string();
    if dir.contains(|c: char| c.is_whitespace() || c == '\'') {
        format!(" --data-dir \"{dir}\"")
    } else {
        format!(" --data-dir {dir}")
    }
}

/// "3 minutes", "2 hours", "5 days": how long ago `at` was.
fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds().max(0);
    let (n, unit) = match secs {
        0..60 => (secs, "second"),
        60..3600 => (secs / 60, "minute"),
        3600..86_400 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// What to do next with a server that listens at `api_url`: create its
/// account while it has none (its setup code is then on disk), else connect
/// the CLI.
pub fn next_step(data_dir: &Path, api_url: &str) -> String {
    match ferry_api::setup::read_code(data_dir) {
        Some(code) => format!("Create your account: {}", ferry_api::setup::setup_url(api_url, &code)),
        None if api_url == DEFAULT_API_URL => "Connect the CLI : ferry login".to_string(),
        None => format!("Connect the CLI : ferry login --server {api_url}"),
    }
}

/// The banner under its title: where the server listens, then what to do next.
pub fn print_summary(data_dir: &Path, ready: &Ready) {
    for line in &ready.summary {
        println!("  {line}");
    }
    println!();
    println!("  {}", next_step(data_dir, &ready.api_url));
    println!();
}

/// How to follow and stop a server that runs in the background.
fn print_commands(hint: &str) {
    println!("  Logs            : ferryd logs -f{hint}");
    println!("  Stop            : ferryd stop{hint}");
    println!();
}

/// The lines of a server's log worth showing when it started fine.
fn is_warning(line: &str) -> bool {
    line.contains(" WARN ") || line.contains(" ERROR ")
}

/// What was appended to the log since `offset`.
fn log_since(path: &Path, offset: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Open the log for appending, after moving a big one out of the way.
fn open_log(data_dir: &Path) -> anyhow::Result<(std::fs::File, u64)> {
    let path = log_path(data_dir);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(&path).with_context(|| format!("opening {}", path.display()))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((file, len))
}

/// `ferryd start`: run `ferryd run <server_args>` detached from this
/// terminal, and return once it listens.
#[cfg(unix)]
pub fn start(data_dir: &Path, server_args: &[OsString], hint: &str) -> anyhow::Result<ExitCode> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    if let Some(server) = running(data_dir)? {
        match server.state {
            Some(ServerState { pid, ready: Some(ready), .. }) => {
                println!();
                println!("  Ferry is already running (pid {pid})");
                print_summary(data_dir, &ready);
            }
            Some(ServerState { pid, .. }) => println!("Ferry is already starting (pid {pid})."),
            None => println!("Ferry is already running with data directory {}.", data_dir.display()),
        }
        println!("  Stop it with `ferryd stop{hint}` to start it with other options.");
        return Ok(ExitCode::SUCCESS);
    }

    let (log, offset) = open_log(data_dir)?;
    let log_file = log_path(data_dir);
    let exe = std::env::current_exe().context("finding the ferryd binary")?;
    let mut command = Command::new(&exe);
    command
        .arg("run")
        .arg("--detached")
        .args(server_args)
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("opening the log")?)
        .stderr(log);
    // A session of its own: closing the terminal (SIGHUP) or Ctrl-C in it
    // (SIGINT to the foreground process group) no longer reaches the server.
    // SAFETY: setsid() is async-signal-safe, which is what the child may call
    // between fork and exec.
    unsafe {
        command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    let mut child = command.spawn().with_context(|| format!("starting {}", exe.display()))?;
    let pid = child.id();

    let started = Instant::now();
    let mut announced = false;
    let ready = loop {
        if let Some(status) = child.try_wait().context("waiting for the server")? {
            // What it logged says why (its own `Error: …` at the end).
            eprint!("{}", log_since(&log_file, offset));
            eprintln!("ferryd: the server did not start ({status}). Its log: {}", log_file.display());
            return Ok(ExitCode::FAILURE);
        }
        if let Some(ServerState { pid: p, ready: Some(ready), .. }) = read_state(data_dir)
            && p == pid
        {
            break ready;
        }
        if started.elapsed() > START_TIMEOUT {
            eprint!("{}", log_since(&log_file, offset));
            anyhow::bail!(
                "the server (pid {pid}) is still starting after {}s. It keeps starting in the background: check \
                 `ferryd status{hint}` and `ferryd logs{hint}`",
                START_TIMEOUT.as_secs()
            );
        }
        if !announced && started.elapsed() > Duration::from_secs(2) {
            eprintln!("Starting Ferry…");
            announced = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    for line in log_since(&log_file, offset).lines().filter(|l| is_warning(l)) {
        eprintln!("{line}");
    }
    println!();
    println!("  Ferry {} is running in the background (pid {pid})", ferry_core::VERSION);
    print_summary(data_dir, &ready);
    print_commands(hint);
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(unix))]
pub fn start(_data_dir: &Path, _server_args: &[OsString], _hint: &str) -> anyhow::Result<ExitCode> {
    anyhow::bail!("starting in the background isn't supported on this platform: use `ferryd run`")
}

/// `ferryd status`: exit code 0 when a server runs, 3 otherwise.
pub fn status(data_dir: &Path, hint: &str) -> anyhow::Result<ExitCode> {
    let Some(server) = running(data_dir)? else {
        println!("Ferry is not running (data directory {}).", data_dir.display());
        return Ok(ExitCode::from(NOT_RUNNING));
    };
    let Some(state) = server.state else {
        println!("Ferry is running with data directory {} (it left no ferryd.json).", data_dir.display());
        return Ok(ExitCode::SUCCESS);
    };
    let how = if state.background { "in the background" } else { "in the foreground" };
    let since = ago(state.started_at, Utc::now());
    println!();
    match &state.ready {
        Some(ready) => {
            println!("  Ferry {} is running {how} (pid {}, for {since})", state.version, state.pid);
            print_summary(data_dir, ready);
        }
        None => {
            println!("  Ferry {} is starting {how} (pid {}, for {since})", state.version, state.pid);
            println!();
        }
    }
    if state.background {
        print_commands(hint);
    }
    Ok(ExitCode::SUCCESS)
}

/// `ferryd stop`: ask the server to shut down (SIGTERM) and wait until it
/// released the data directory.
#[cfg(unix)]
pub fn stop(data_dir: &Path) -> anyhow::Result<ExitCode> {
    let Some(server) = running(data_dir)? else {
        println!("Ferry is not running (data directory {}).", data_dir.display());
        return Ok(ExitCode::SUCCESS);
    };
    let Some(state) = server.state else {
        anyhow::bail!(
            "a server runs with data directory {} but left no ferryd.json, so its process is unknown: stop it where \
             it was started",
            data_dir.display()
        );
    };
    let pid = libc::pid_t::try_from(state.pid).context("the server's process id")?;
    if !state.background {
        println!(
            "This server runs in the foreground. If a service manager started it (systemd, launchd), stop it there \
             too, or it may start it again."
        );
    }
    // SAFETY: kill() only sends a signal; the pid is the one the process that
    // holds the data directory's lock wrote.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == -1 {
        let e = std::io::Error::last_os_error();
        return Err(e).with_context(|| format!("stopping the server (pid {pid})"));
    }
    println!("Stopping Ferry (pid {pid})…");
    let started = Instant::now();
    while is_locked(data_dir)? {
        if started.elapsed() > STOP_TIMEOUT {
            anyhow::bail!(
                "the server (pid {pid}) is still shutting down after {}s. Running `ferryd stop` again forces it to \
                 exit at once, like a second Ctrl-C",
                STOP_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("Ferry stopped.");
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(unix))]
pub fn stop(_data_dir: &Path) -> anyhow::Result<ExitCode> {
    anyhow::bail!("`ferryd stop` isn't supported on this platform: stop the server where it was started")
}

/// The last `lines` lines of `text` (all of it when it has fewer).
fn tail(text: &str, lines: usize) -> &str {
    if lines == 0 {
        return "";
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    match body.rmatch_indices('\n').nth(lines - 1) {
        Some((i, _)) => &text[i + 1..],
        None => text,
    }
}

/// `ferryd logs`: the end of `ferryd.log`, and with `follow` what is written
/// to it afterwards (until Ctrl-C).
pub fn logs(data_dir: &Path, lines: usize, follow: bool) -> anyhow::Result<ExitCode> {
    let path = log_path(data_dir);
    let len = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!(
            "no log at {}: only a server started in the background (`ferryd start`) writes one. A server in the \
             foreground logs to its terminal or its service manager (e.g. `journalctl -u ferry`)",
            path.display()
        ),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if let Some(Running { state: Some(ServerState { background: false, .. }) }) = running(data_dir)? {
        eprintln!(
            "The server that runs now is in the foreground and logs where it was started: this is the log of an \
             earlier server."
        );
    }
    let from = len.saturating_sub(LOG_TAIL_BYTES);
    let text = log_since(&path, from);
    // A read that starts in the middle of the file starts in the middle of a line.
    let text = if from > 0 { text.split_once('\n').map(|(_, rest)| rest).unwrap_or_default() } else { &text };
    let mut out = std::io::stdout().lock();
    out.write_all(tail(text, lines).as_bytes())?;
    out.flush()?;
    if !follow {
        return Ok(ExitCode::SUCCESS);
    }
    let mut pos = len;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        // By path each time: `ferryd start` may have moved the file away.
        let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else { continue };
        if len < pos {
            pos = 0;
        }
        if len > pos {
            let chunk = log_since(&path, pos);
            pos += chunk.len() as u64;
            out.write_all(chunk.as_bytes())?;
            out.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferryd-daemon-{}", ferry_core::ids::random_secret(8)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nothing_runs_in_an_empty_data_dir() {
        let dir = temp_dir();
        assert_eq!(running(&dir).unwrap(), None);
        // A state file alone (a server that was killed) doesn't count.
        let state = StateFile::create(&dir, true).unwrap();
        assert!(state_path(&dir).exists());
        assert_eq!(running(&dir).unwrap(), None);
        std::mem::forget(state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_holder_is_the_running_server() {
        let dir = temp_dir();
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(lock_path(&dir)).unwrap();
        lock.try_lock().unwrap();
        assert_eq!(running(&dir).unwrap(), Some(Running { state: None }), "locked, no state file");

        let mut state = StateFile::create(&dir, true).unwrap();
        let found = running(&dir).unwrap().unwrap().state.unwrap();
        assert_eq!(found.pid, std::process::id());
        assert_eq!(found.version, ferry_core::VERSION);
        assert!(found.background);
        assert_eq!(found.ready, None);

        let ready = Ready { api_url: "http://127.0.0.1:7878".into(), summary: vec!["Data : /x".into()] };
        state.set_ready(ready.clone()).unwrap();
        assert_eq!(running(&dir).unwrap().unwrap().state.unwrap().ready, Some(ready));

        // Stopping removes the file, then releases the lock.
        drop(state);
        assert!(!state_path(&dir).exists());
        drop(lock);
        assert_eq!(running(&dir).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_state_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let _state = StateFile::create(&dir, false).unwrap();
        let mode = std::fs::metadata(state_path(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc\n");
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
        assert_eq!(tail("a\nb\nc\n", 3), "a\nb\nc\n");
        assert_eq!(tail("a\nb\nc\n", 10), "a\nb\nc\n");
        assert_eq!(tail("a\n", 1), "a\n");
        assert_eq!(tail("", 5), "");
        assert_eq!(tail("a\nb\n", 0), "");
    }

    #[test]
    fn log_since_reads_what_was_appended() {
        let dir = temp_dir();
        let path = log_path(&dir);
        std::fs::write(&path, "old\n").unwrap();
        let (mut log, offset) = open_log(&dir).unwrap();
        assert_eq!(offset, 4);
        log.write_all(b"2026-10-01T10:00:00Z  WARN low disk\n2026-10-01T10:00:01Z  INFO listening\n").unwrap();
        let chunk = log_since(&path, offset);
        assert_eq!(chunk.lines().count(), 2);
        assert_eq!(
            chunk.lines().filter(|l| is_warning(l)).collect::<Vec<_>>(),
            ["2026-10-01T10:00:00Z  WARN low disk"]
        );
        assert_eq!(log_since(&dir.join("missing.log"), 0), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_big_log_is_moved_away_on_start() {
        let dir = temp_dir();
        let path = log_path(&dir);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(LOG_ROTATE_BYTES + 1).unwrap();
        drop(file);
        let (_log, offset) = open_log(&dir).unwrap();
        assert_eq!(offset, 0);
        assert_eq!(std::fs::metadata(dir.join("ferryd.log.1")).unwrap().len(), LOG_ROTATE_BYTES + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hints_name_a_data_dir_that_is_not_the_default() {
        assert_eq!(data_dir_hint(Path::new("./ferry-data"), true), "");
        assert_eq!(data_dir_hint(Path::new("/var/lib/ferry"), false), " --data-dir /var/lib/ferry");
        assert_eq!(data_dir_hint(Path::new("/tmp/my ferry"), false), " --data-dir \"/tmp/my ferry\"");
    }

    #[test]
    fn ago_is_readable() {
        let now = Utc::now();
        assert_eq!(ago(now, now), "0 seconds");
        assert_eq!(ago(now - chrono::Duration::seconds(1), now), "1 second");
        assert_eq!(ago(now - chrono::Duration::seconds(150), now), "2 minutes");
        assert_eq!(ago(now - chrono::Duration::hours(1), now), "1 hour");
        assert_eq!(ago(now - chrono::Duration::days(3), now), "3 days");
        assert_eq!(ago(now + chrono::Duration::seconds(30), now), "0 seconds", "a clock that went back");
    }
}
