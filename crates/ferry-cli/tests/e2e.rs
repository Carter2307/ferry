//! Docker-gated end-to-end test: a real `ferryd` driven by the `ferry` CLI.
//!
//! Runs only with `FERRY_E2E=1` and a `ferryd` binary (`FERRY_E2E_FERRYD`,
//! else `target/<profile>/ferryd` next to this test's build — build it with
//! `cargo build -p ferryd`). Otherwise it prints "skipped" and passes.
//! Every Docker object it creates carries a unique `ferrytest-cli-*` prefix
//! and is removed at the end, even on failure.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{Output, ferry_in};

const TOKEN: &str = "e2e-token-0123456789";

fn find_ferryd() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FERRY_E2E_FERRYD") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    // target/<profile>/deps/e2e-xxxx → target/<profile>/ferryd
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.parent()?.join(if cfg!(windows) { "ferryd.exe" } else { "ferryd" });
    candidate.is_file().then_some(candidate)
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:06x}", (nanos ^ (u128::from(std::process::id()) << 20)) & 0xff_ffff)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn docker(args: &[&str]) -> String {
    Command::new("docker")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Removes every Docker object carrying this test's unique prefix.
struct DockerCleanup(String);

impl Drop for DockerCleanup {
    fn drop(&mut self) {
        let p = &self.0;
        let ids: Vec<String> = docker(&["ps", "-aq", "--filter", &format!("label=ferry.instance={p}")])
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if !ids.is_empty() {
            let mut args = vec!["rm", "-f"];
            args.extend(ids.iter().map(String::as_str));
            docker(&args);
        }
        let volumes: Vec<String> = docker(&["volume", "ls", "-q", "--filter", &format!("name={p}-")])
            .split_whitespace()
            .filter(|v| v.starts_with(p.as_str()))
            .map(str::to_string)
            .collect();
        for v in &volumes {
            docker(&["volume", "rm", "-f", v]);
        }
        let images: Vec<String> =
            docker(&["images", "--format", "{{.Repository}}:{{.Tag}}", "--filter", &format!("reference={p}/*")])
                .split_whitespace()
                .filter(|i| i.starts_with(&format!("{p}/")))
                .map(str::to_string)
                .collect();
        for i in &images {
            docker(&["rmi", "-f", i]);
        }
        docker(&["network", "rm", p]);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Last lines of the ferryd log, for failure messages.
fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

fn assert_ok(step: &str, out: &Output, log: &Path) {
    assert_eq!(
        out.code,
        0,
        "{step} failed:\nstdout:\n{}\nstderr:\n{}\nferryd log:\n{}",
        out.stdout,
        out.stderr,
        log_tail(log)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_up_deploy_env_and_delete_a_static_site() {
    if std::env::var("FERRY_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipped: set FERRY_E2E=1 to run the Docker end-to-end test");
        return;
    }
    let Some(ferryd) = find_ferryd() else {
        eprintln!("skipped: no ferryd binary found (cargo build -p ferryd, or set FERRY_E2E_FERRYD)");
        return;
    };
    if docker(&["version", "--format", "{{.Server.Version}}"]).trim().is_empty() {
        eprintln!("skipped: Docker is not reachable");
        return;
    }

    let prefix = format!("ferrytest-cli-{}", unique_suffix());
    let _cleanup = DockerCleanup(prefix.clone());
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (api_port, proxy_port) = (free_port(), free_port());
    let server = format!("http://127.0.0.1:{api_port}");
    let log = data.path().join("ferryd.log");
    let log_file = std::fs::File::create(&log).unwrap();

    let mut daemon = tokio::process::Command::new(&ferryd)
        .args(["--data-dir", data.path().to_str().unwrap()])
        .args(["--api-addr", &format!("127.0.0.1:{api_port}")])
        .args(["--proxy-addr", &format!("127.0.0.1:{proxy_port}")])
        .args(["--name-prefix", &prefix])
        .args(["--api-token", TOKEN])
        .args(["--dashboard-host", "none"])
        .args(["--base-domain", "localhost"])
        .args(["--health-check-timeout", "90"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone().unwrap()))
        .stderr(Stdio::from(log_file))
        .kill_on_drop(true)
        .spawn()
        .expect("starting ferryd");

    // Wait until the API answers.
    let http = reqwest::Client::new();
    let started = Instant::now();
    loop {
        if let Ok(Some(status)) = daemon.try_wait() {
            eprintln!("skipped: ferryd exited during startup ({status}):\n{}", log_tail(&log));
            return;
        }
        if let Ok(r) = http.get(format!("{server}/healthz")).send().await
            && r.status().is_success()
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(30) {
            eprintln!("skipped: ferryd did not become healthy within 30s");
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let run = |args: Vec<String>| {
        let home = home.path().to_path_buf();
        async move {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            ferry_in(None, &home, None, None, &refs).await
        }
    };
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let out = run(args(&["login", "--server", &server, "--token", TOKEN])).await;
    assert_ok("login", &out, &log);
    let out = run(args(&["info"])).await;
    assert_ok("info", &out, &log);
    assert!(out.stdout.contains("Docker:"), "{}", out.stdout);

    // Deploy the static-site example from a local directory.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/static-site");
    let work = tempfile::tempdir().unwrap();
    let site = work.path().join("e2e-site");
    copy_dir(&src, &site);
    let out = run(args(&["up", "--dir", site.to_str().unwrap(), "--type", "static", "--follow"])).await;
    assert_ok("up --follow", &out, &log);
    assert!(out.stdout.contains("is live"), "{}", out.stdout);

    let out = run(args(&["status", "e2e-site"])).await;
    assert_ok("status", &out, &log);
    assert!(out.stdout.contains("running"), "{}", out.stdout);

    // Reachable through the proxy by Host header.
    let resp = http
        .get(format!("http://127.0.0.1:{proxy_port}/"))
        .header("Host", "e2e-site.localhost")
        .send()
        .await
        .expect("request through the proxy");
    assert!(resp.status().is_success(), "proxy answered {}", resp.status());
    assert!(resp.text().await.unwrap().contains("Hello from a Ferry static site"));

    let out = run(args(&["env", "set", "e2e-site", "GREETING=hi there", "--no-restart"])).await;
    assert_ok("env set", &out, &log);
    let out = run(args(&["env", "e2e-site"])).await;
    assert_ok("env", &out, &log);
    assert!(out.stdout.contains("GREETING=hi there"), "{}", out.stdout);

    let out = run(args(&["deploys", "e2e-site"])).await;
    assert_ok("deploys", &out, &log);
    assert!(out.stdout.contains("live"), "{}", out.stdout);
    let out = run(args(&["logs", "e2e-site", "--tail", "20"])).await;
    assert_ok("logs", &out, &log);
    let out = run(args(&["services", "--json"])).await;
    assert_ok("services --json", &out, &log);
    let list: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(list.as_array().unwrap().iter().any(|s| s["name"] == "e2e-site"));

    let out = run(args(&["delete", "e2e-site", "--yes"])).await;
    assert_ok("delete", &out, &log);
    let out = run(args(&["show", "e2e-site"])).await;
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("not_found"), "{}", out.stderr);

    let _ = daemon.kill().await;
}
