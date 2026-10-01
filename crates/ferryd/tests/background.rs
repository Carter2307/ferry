//! `ferryd start` / `status` / `logs` / `stop` against the real binary.
//!
//! The tests of a server that never gets to listen run everywhere. The full
//! cycle needs Docker and runs only with `FERRY_E2E=1` (otherwise it prints
//! "skipped" and passes); every Docker object it creates carries a unique
//! `ferrytest-bg-*` prefix and is removed at the end, even on failure.
#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const FERRYD: &str = env!("CARGO_BIN_EXE_ferryd");

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

fn ferryd(args: &[&str]) -> Output {
    let out = Command::new(FERRYD).args(args).stdin(Stdio::null()).output().expect("running ferryd");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:06x}", (nanos ^ (u128::from(std::process::id()) << 20)) & 0xff_ffff)
}

fn docker(args: &[&str]) {
    let _ = Command::new("docker").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Stops the server of a data directory and removes the Docker objects of
/// its prefix, whatever the test did.
struct Cleanup<'a> {
    data_dir: &'a str,
    prefix: String,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        ferryd(&["stop", "--data-dir", self.data_dir]);
        docker(&["volume", "rm", "-f", &format!("{}-owner", self.prefix)]);
        docker(&["network", "rm", &self.prefix]);
    }
}

fn healthz(port: u16) -> String {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connecting to the API");
    stream.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer
}

#[test]
fn status_and_stop_without_a_server() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_str().unwrap();
    let status = ferryd(&["status", "--data-dir", dir]);
    assert_eq!(status.code, 3, "{}", status.stderr);
    assert!(status.stdout.contains("Ferry is not running"), "{}", status.stdout);
    // Nothing to stop is not an error.
    let stop = ferryd(&["stop", "--data-dir", dir]);
    assert_eq!(stop.code, 0, "{}", stop.stderr);
    assert!(stop.stdout.contains("Ferry is not running"), "{}", stop.stdout);
    // Only a server started in the background has a log.
    let logs = ferryd(&["logs", "--data-dir", dir]);
    assert_eq!(logs.code, 1);
    assert!(logs.stderr.contains("no log at") && logs.stderr.contains("ferryd start"), "{}", logs.stderr);
    // A data directory that doesn't exist is the same.
    let missing = data.path().join("missing");
    assert_eq!(ferryd(&["status", "--data-dir", missing.to_str().unwrap()]).code, 3);
    assert!(!missing.exists(), "`status` must not create the data directory");
}

/// The server fails before it listens (its API port is taken, which it checks
/// before it needs Docker): `start` says why in the terminal and exits 1.
#[test]
fn a_start_that_fails_says_why() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_str().unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api_addr = taken.local_addr().unwrap().to_string();
    let proxy_addr = format!("127.0.0.1:{}", free_port());
    let start = ferryd(&[
        "start",
        "--data-dir",
        dir,
        "--api-addr",
        &api_addr,
        "--proxy-addr",
        &proxy_addr,
        "--oom-score-adj=0",
    ]);
    assert_eq!(start.code, 1, "stdout:\n{}\nstderr:\n{}", start.stdout, start.stderr);
    assert!(start.stderr.contains(&format!("cannot listen on {api_addr} (API address)")), "{}", start.stderr);
    assert!(start.stderr.contains("the server did not start"), "{}", start.stderr);
    assert!(!start.stdout.contains("is running"), "{}", start.stdout);
    // Nothing is left running, and the log kept the error.
    assert_eq!(ferryd(&["status", "--data-dir", dir]).code, 3);
    let logs = ferryd(&["logs", "--data-dir", dir]);
    assert_eq!(logs.code, 0, "{}", logs.stderr);
    assert!(logs.stdout.contains("cannot listen on"), "{}", logs.stdout);
    assert!(!logs.stdout.contains('\u{1b}'), "no color codes in the log file: {}", logs.stdout);
    assert!(!Path::new(dir).join("ferryd.json").exists());
}

#[test]
fn start_status_logs_stop() {
    if std::env::var("FERRY_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipped: set FERRY_E2E=1 to run the Docker end-to-end test");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_str().unwrap();
    let prefix = format!("ferrytest-bg-{}", unique_suffix());
    let _cleanup = Cleanup { data_dir: dir, prefix: prefix.clone() };
    let api_port = free_port();
    let api_addr = format!("127.0.0.1:{api_port}");
    let proxy_addr = format!("127.0.0.1:{}", free_port());
    let server = ["--data-dir", dir, "--api-addr", &api_addr, "--proxy-addr", &proxy_addr, "--name-prefix", &prefix];
    let server = [&server[..], &["--dashboard-host", "none", "--oom-score-adj=0"]].concat();

    // `start` returns once the server listens.
    let start = ferryd(&[&["start"], &server[..]].concat());
    if start.code != 0 && start.stderr.contains("cannot connect to Docker") {
        eprintln!("skipped: no Docker daemon");
        return;
    }
    assert_eq!(start.code, 0, "stdout:\n{}\nstderr:\n{}", start.stdout, start.stderr);
    assert!(start.stdout.contains("is running in the background (pid "), "{}", start.stdout);
    assert!(start.stdout.contains(&format!("Dashboard + API : http://{api_addr}")), "{}", start.stdout);
    assert!(start.stdout.contains(&format!("ferryd stop --data-dir {dir}")), "{}", start.stdout);
    assert!(healthz(api_port).contains("200 OK"), "the API answers");

    let status = ferryd(&["status", "--data-dir", dir]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    assert!(status.stdout.contains("is running in the background (pid "), "{}", status.stdout);
    assert!(status.stdout.contains(&format!("http://{api_addr}")), "{}", status.stdout);

    // A second `start` leaves the server alone.
    let again = ferryd(&[&["start"], &server[..]].concat());
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert!(again.stdout.contains("Ferry is already running (pid "), "{}", again.stdout);
    // So does a server in the foreground: the data directory is taken.
    let run = ferryd(&[&["run"], &server[..]].concat());
    assert_eq!(run.code, 1);
    assert!(run.stderr.contains("another ferryd is already running"), "{}", run.stderr);

    let logs = ferryd(&["logs", "--data-dir", dir, "-n", "1000"]);
    assert_eq!(logs.code, 0, "{}", logs.stderr);
    assert!(logs.stdout.contains("engine started"), "{}", logs.stdout);
    assert_eq!(ferryd(&["logs", "--data-dir", dir, "-n", "2"]).stdout.lines().count(), 2);

    let stop = ferryd(&["stop", "--data-dir", dir]);
    assert_eq!(stop.code, 0, "{}", stop.stderr);
    assert!(stop.stdout.contains("Ferry stopped."), "{}", stop.stdout);
    assert_eq!(ferryd(&["status", "--data-dir", dir]).code, 3);
    assert!(std::net::TcpStream::connect(("127.0.0.1", api_port)).is_err(), "the API no longer listens");
    assert!(!Path::new(dir).join("ferryd.json").exists());
}
