//! Children a killed server left behind.
//!
//! Every `git` / `docker` child of a build leads its own process group (see
//! [`crate::process`]). While it runs, a one-line record of it sits in the
//! build's scratch directory (`<builds_dir>/<deploy_id>/children/<pid>`):
//! `<pid> <start, unix seconds> <program>`. A server stopped normally kills
//! its children and removes the records; one killed with SIGKILL cannot (on
//! Linux the direct child gets SIGKILL through `PR_SET_PDEATHSIG`, on macOS
//! nothing stops it): a `docker build` then runs to completion, re-parented
//! to init. [`reap`] kills such groups on the next start.
//!
//! A recorded pid may since have been reused by an unrelated process, so a
//! group is only killed when its leader is still the recorded child: it
//! leads its own process group, runs the recorded program, started no later
//! than the record was written, and is not a child of this process.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sub-directory of a build's scratch directory holding its child records.
pub(crate) const CHILDREN_DIR: &str = "children";

/// Tolerance on start times (`ps` reports whole seconds; the record is
/// written just after the spawn).
const START_SLACK_SECS: u64 = 5;

/// Linux truncates `comm` to 15 bytes.
const COMM_LEN: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub pid: u32,
    /// Unix seconds when the child was spawned.
    pub started: u64,
    /// Basename of the program (`git`, `docker`).
    pub program: String,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// Record a child that was just spawned. Returns the record's path (removed
/// by the caller once the child has been reaped).
pub(crate) fn write_record(dir: &Path, pid: u32, program: &OsStr) -> io::Result<PathBuf> {
    let program = program.to_string_lossy();
    let program: String = basename(&program).chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    fs::create_dir_all(dir)?;
    let path = dir.join(pid.to_string());
    let mut f = fs::File::create(&path)?;
    writeln!(f, "{pid} {} {program}", now_secs())?;
    Ok(path)
}

pub(crate) fn parse_record(s: &str) -> Option<Record> {
    let mut parts = s.split_whitespace();
    let pid: u32 = parts.next()?.parse().ok()?;
    let started: u64 = parts.next()?.parse().ok()?;
    let program = parts.next()?.to_string();
    (pid > 1 && parts.next().is_none()).then_some(Record { pid, started, program })
}

/// What `ps` says about a live process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcInfo {
    pub pgid: u32,
    pub ppid: u32,
    /// Seconds since it started.
    pub elapsed: u64,
    /// Command name (a full path on macOS, 15 bytes at most on Linux).
    pub comm: String,
}

/// `ps -o etime=`: `[[dd-]hh:]mm:ss`.
pub(crate) fn parse_etime(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, clock) = match s.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, s),
    };
    let parts: Vec<u64> = clock.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [m, s] => (0, *m, *s),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    Some(((days * 24 + h) * 60 + m) * 60 + sec)
}

/// One line of `ps -o pgid=,ppid=,etime=,comm=`.
pub(crate) fn parse_ps_line(line: &str) -> Option<ProcInfo> {
    let mut rest = line.trim_start();
    let mut next = || {
        let end = rest.find(char::is_whitespace)?;
        let field = &rest[..end];
        rest = rest[end..].trim_start();
        Some(field)
    };
    let pgid = next()?.parse().ok()?;
    let ppid = next()?.parse().ok()?;
    let elapsed = parse_etime(next()?)?;
    let comm = rest.trim_end().to_string();
    (!comm.is_empty()).then_some(ProcInfo { pgid, ppid, elapsed, comm })
}

/// Look a process up with `ps` (`None`: gone, or `ps` unavailable).
fn proc_info(pid: u32) -> Option<ProcInfo> {
    let out = std::process::Command::new("ps")
        .args(["-o", "pgid=,ppid=,etime=,comm=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().find_map(parse_ps_line)
}

/// Is `info` (the process now holding `record.pid`) still the recorded child?
pub(crate) fn still_the_recorded_child(record: &Record, info: &ProcInfo, now: u64, self_pid: u32) -> bool {
    let started = now.saturating_sub(info.elapsed);
    let comm = basename(&info.comm);
    let same_program = !record.program.is_empty()
        && comm.bytes().take(COMM_LEN).eq(record.program.bytes().take(COMM_LEN))
        && comm.len().min(COMM_LEN) == record.program.len().min(COMM_LEN);
    info.pgid == record.pid
        && info.ppid != self_pid
        && same_program
        && started <= record.started.saturating_add(START_SLACK_SECS)
}

/// Kill the process groups recorded under `<builds_dir>/*/children/` whose
/// leader is still the recorded child; remove every record. Returns the
/// number of groups killed.
#[cfg(unix)]
pub(crate) fn reap(builds_dir: &Path) -> usize {
    let Ok(builds) = fs::read_dir(builds_dir) else { return 0 };
    let self_pid = std::process::id();
    let mut killed = 0;
    for build in builds.flatten() {
        if !build.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(records) = fs::read_dir(build.path().join(CHILDREN_DIR)) else { continue };
        for entry in records.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let record = fs::read_to_string(&path).ok().and_then(|s| parse_record(&s));
            if let Some(record) = record
                && let Some(info) = proc_info(record.pid)
                && still_the_recorded_child(&record, &info, now_secs(), self_pid)
                && let Ok(pgid) = i32::try_from(record.pid)
            {
                // SAFETY: kill(2) has no memory-safety preconditions; the
                // negative pid targets the verified orphaned group only.
                let rc = unsafe { libc::kill(-pgid, libc::SIGKILL) };
                if rc == 0 {
                    killed += 1;
                    tracing::info!(
                        pid = record.pid,
                        program = %record.program,
                        build = %build.file_name().to_string_lossy(),
                        "killed a build process left running by a previous server"
                    );
                }
            }
            let _ = fs::remove_file(&path);
        }
    }
    killed
}

#[cfg(not(unix))]
pub(crate) fn reap(_builds_dir: &Path) -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let path = write_record(&d.path().join(CHILDREN_DIR), 4242, OsStr::new("/usr/local/bin/docker")).unwrap();
        assert!(path.ends_with("children/4242"));
        let r = parse_record(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!((r.pid, r.program.as_str()), (4242, "docker"));
        assert!(r.started.abs_diff(now_secs()) <= 1);
        assert_eq!(parse_record("12 34"), None);
        assert_eq!(parse_record("1 34 init"), None);
        assert_eq!(parse_record("x 34 git"), None);
        assert_eq!(parse_record("12 34 git extra"), None);
    }

    #[test]
    fn parses_ps_output() {
        assert_eq!(parse_etime("00:05"), Some(5));
        assert_eq!(parse_etime(" 01:02:03 "), Some(3723));
        assert_eq!(parse_etime("2-01:00:00"), Some(2 * 86400 + 3600));
        assert_eq!(parse_etime("11-17:30:44"), Some(11 * 86400 + 17 * 3600 + 30 * 60 + 44));
        assert_eq!(parse_etime("soon"), None);
        assert_eq!(parse_etime("1:2:3:4"), None);
        assert_eq!(
            parse_ps_line(" 6408  6407 00:07 /Applications/Docker Desktop.app/bin/docker\n"),
            Some(ProcInfo {
                pgid: 6408,
                ppid: 6407,
                elapsed: 7,
                comm: "/Applications/Docker Desktop.app/bin/docker".into()
            })
        );
        assert_eq!(
            parse_ps_line("    1     0 11-17:30:44 launchd"),
            Some(ProcInfo { pgid: 1, ppid: 0, elapsed: 1_013_444, comm: "launchd".into() })
        );
        assert_eq!(parse_ps_line(""), None);
        assert_eq!(parse_ps_line("1 2 00:01"), None);
    }

    /// Runs only inside [`sigkilled_server_orphans_are_reaped`]'s child
    /// process: a "server" with a build child running, killed with SIGKILL.
    #[cfg(unix)]
    #[test]
    fn orphan_helper() {
        let Some(records) = std::env::var_os("FERRY_BUILD_ORPHAN_HELPER") else { return };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        rt.block_on(async move {
            let mut cmd = tokio::process::Command::new("sleep");
            cmd.arg("60");
            let cancel = ferry_core::CancellationToken::new();
            let run = crate::process::run_streaming(cmd, &cancel, 0, |_| {}, |_| {});
            let _ = crate::process::record_children_in(PathBuf::from(records), run).await;
        });
    }

    #[cfg(unix)]
    #[test]
    fn sigkilled_server_orphans_are_reaped() {
        use std::time::{Duration, Instant};
        let builds = tempfile::tempdir().unwrap();
        let records = builds.path().join("dep-1").join(CHILDREN_DIR);
        let mut server = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "orphans::tests::orphan_helper", "--nocapture", "--test-threads=1"])
            .env("FERRY_BUILD_ORPHAN_HELPER", &records)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let pid: i32 = loop {
            let found = fs::read_dir(&records).ok().and_then(|mut rd| rd.next()).and_then(|e| e.ok());
            if let Some(pid) = found.and_then(|e| e.file_name().to_str().and_then(|n| n.parse().ok())) {
                break pid;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "the build child was never recorded");
            std::thread::sleep(Duration::from_millis(20));
        };
        // `kill -9` the server: nothing of it runs any more.
        server.kill().unwrap();
        server.wait().unwrap();

        let reaped = reap(builds.path());
        // Linux already killed the direct child (PR_SET_PDEATHSIG; it may
        // still be a zombie); elsewhere it kept running until reaped.
        if cfg!(target_os = "linux") {
            assert!(reaped <= 1, "{reaped}");
        } else {
            assert_eq!(reaped, 1);
        }
        let start = Instant::now();
        // SAFETY: signal 0 only checks that the pid exists.
        while unsafe { libc::kill(pid, 0) } == 0 && proc_info(pid as u32).is_some_and(|i| !i.comm.contains("defunct")) {
            assert!(start.elapsed() < Duration::from_secs(5), "orphan {pid} survived the reaping");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(fs::read_dir(&records).unwrap().next().is_none(), "records are removed");
        // Nothing left to do the second time.
        assert_eq!(reap(builds.path()), 0);
    }

    #[cfg(unix)]
    #[test]
    fn reused_pids_and_live_children_are_left_alone() {
        let builds = tempfile::tempdir().unwrap();
        let records = builds.path().join("dep-2").join(CHILDREN_DIR);
        fs::create_dir_all(&records).unwrap();
        // Our own live child (a running build of this server).
        let mut own = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        write_record(&records, own.id(), OsStr::new("sleep")).unwrap();
        // A pid now held by another program.
        fs::write(records.join("1"), "1 0 docker\n").unwrap();
        fs::write(records.join(std::process::id().to_string()), format!("{} 0 docker\n", std::process::id())).unwrap();
        // A dead pid and garbage.
        fs::write(records.join("999999"), "999999 0 docker\n").unwrap();
        fs::write(records.join("junk"), "not a record").unwrap();
        assert_eq!(reap(builds.path()), 0);
        assert!(own.try_wait().unwrap().is_none(), "a live child of this process was killed");
        own.kill().unwrap();
        own.wait().unwrap();
        assert!(fs::read_dir(&records).unwrap().next().is_none());
        // No builds directory at all.
        assert_eq!(reap(&builds.path().join("missing")), 0);
    }

    #[test]
    fn only_the_recorded_child_is_reaped() {
        let now = 1_000_000;
        let record = Record { pid: 500, started: now - 60, program: "docker".into() };
        let orphan = ProcInfo { pgid: 500, ppid: 1, elapsed: 60, comm: "/usr/local/bin/docker".into() };
        assert!(still_the_recorded_child(&record, &orphan, now, 77));
        // Linux truncates comm to 15 bytes.
        let long = Record { program: "docker-buildx-plugin".into(), ..record.clone() };
        assert!(still_the_recorded_child(
            &long,
            &ProcInfo { comm: "docker-buildx-p".into(), ..orphan.clone() },
            now,
            77
        ));
        // pid reused by a process started after the record was written
        assert!(!still_the_recorded_child(&record, &ProcInfo { elapsed: 10, ..orphan.clone() }, now, 77));
        // another program, not a group leader, or our own live child
        assert!(!still_the_recorded_child(&record, &ProcInfo { comm: "/bin/zsh".into(), ..orphan.clone() }, now, 77));
        assert!(!still_the_recorded_child(&record, &ProcInfo { comm: "dockerd".into(), ..orphan.clone() }, now, 77));
        assert!(!still_the_recorded_child(&record, &ProcInfo { pgid: 499, ..orphan.clone() }, now, 77));
        assert!(!still_the_recorded_child(&record, &ProcInfo { ppid: 77, ..orphan.clone() }, now, 77));
    }
}
