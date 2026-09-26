//! Docker-gated regression tests for the web-round build findings: an
//! explicit publish directory `.`, npm install errors that name the missing
//! package, and `docker build` processes left running by a server killed
//! with SIGKILL. Run with
//! `FERRY_E2E=1 cargo test -p ferry-build --test webround_regressions_e2e`;
//! without it every test returns early ("skipped").
//!
//! Images are tagged `ferryweb-build-<random>/<name>:<deploy>`; images and
//! containers are removed afterwards, even when a test fails.
#![cfg(unix)]

mod common;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::*;
use ferry_build::{BuildOutput, BuildRequest, BuildSource, Builder};
use ferry_core::naming::Naming;
use ferry_core::{CancellationToken, Error, LogSink, Result, ServiceType, ids};

fn e2e_enabled() -> bool {
    if std::env::var("FERRY_E2E").as_deref() == Ok("1") {
        true
    } else {
        eprintln!("skipped: set FERRY_E2E=1 to run Docker end-to-end tests");
        false
    }
}

/// Per-test Docker namespace; removes every image and container it created on drop.
struct Env {
    prefix: String,
    naming: Naming,
    _dir: tempfile::TempDir,
    root: PathBuf,
    builder: Builder,
    tags: Mutex<Vec<String>>,
}

fn new_builder(root: &Path) -> Builder {
    Builder::new(root.join("data/builds"), root.join("data/repos"), "docker".to_string())
}

impl Env {
    fn new() -> Env {
        let prefix = format!("ferryweb-build-{}", ids::random_secret(8));
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        Env {
            naming: Naming::new(&prefix),
            prefix,
            builder: new_builder(&root),
            _dir: dir,
            root,
            tags: Mutex::new(Vec::new()),
        }
    }

    /// Upload `files` as `ferry up` would (a rooted archive).
    fn archive(&self, name: &str, files: &[(&str, &str)]) -> BuildSource {
        let src = self.root.join("src").join(name);
        for (path, contents) in files {
            let p = src.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }
        let archive = self.root.join(format!("{name}-{}.tar.gz", ids::random_secret(6)));
        tar_gz(&src, None, &archive);
        BuildSource::Archive { path: archive }
    }

    fn request(&self, name: &str, service_type: ServiceType, source: BuildSource) -> BuildRequest {
        let service_id = ids::new_id(ids::SERVICE);
        let deploy_id = ids::new_id(ids::DEPLOY);
        let tag = self.naming.image_tag(name, &deploy_id);
        self.tags.lock().unwrap().push(tag.clone());
        let mut req = request(&service_id, &deploy_id, name, service_type, source, &tag);
        req.labels = self.naming.service_labels(&service_id, &deploy_id);
        req
    }

    async fn build(&self, req: &BuildRequest) -> (Result<BuildOutput>, Vec<String>) {
        let (logs, mut rx) = LogSink::channel();
        let res =
            tokio::time::timeout(Duration::from_secs(600), self.builder.build(req, &logs, &CancellationToken::new()))
                .await
                .expect("build timed out");
        (res, drain(&mut rx))
    }

    /// `docker run -d` the image with port 80 published on loopback.
    fn run_http(&self, tag: &str) -> u16 {
        let out =
            docker(&["run", "-d", "-p", "127.0.0.1::80", "--label", &format!("ferry.instance={}", self.prefix), tag]);
        let id = out.trim().to_string();
        let port = docker(&["port", &id, "80/tcp"]);
        port.lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.trim().parse().ok())
            .expect("published port")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Ok(out) = Command::new("docker")
            .args(["ps", "-aq", "--filter"])
            .arg(format!("label=ferry.instance={}", self.prefix))
            .output()
        {
            for id in String::from_utf8_lossy(&out.stdout).lines() {
                let _ = Command::new("docker").args(["rm", "-f", id]).output();
            }
        }
        let mut tags = self.tags.lock().map(|t| t.clone()).unwrap_or_default();
        if let Ok(out) = Command::new("docker")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}", "--filter"])
            .arg(format!("reference={}/*", self.prefix))
            .output()
        {
            tags.extend(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string));
        }
        for tag in tags {
            let _ = Command::new("docker").args(["image", "rm", "-f", &tag]).output();
        }
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().expect("docker runs");
    assert!(out.status.success(), "docker {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// (status, body) of `GET path` on 127.0.0.1:port (retrying while the server starts).
fn http_get(port: u16, path: &str) -> (u16, String) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let attempt = (|| -> std::io::Result<(u16, String)> {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port))?;
            s.set_read_timeout(Some(Duration::from_secs(5)))?;
            write!(s, "GET {path} HTTP/1.0\r\nHost: site.localhost\r\n\r\n")?;
            let mut buf = String::new();
            s.read_to_string(&mut buf)?;
            let code = buf.split_whitespace().nth(1).and_then(|c| c.parse().ok());
            let body = buf.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
            code.map(|c| (c, body)).ok_or_else(|| std::io::Error::other(buf.clone()))
        })();
        match attempt {
            Ok(r) => return r,
            Err(e) if Instant::now() > deadline => panic!("GET {path} on port {port}: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Regression: a vite-like app (build → dist/, root index.html) with
/// publish_dir '.' was served from dist/ anyway.
#[tokio::test]
async fn explicit_root_publish_dir_is_served_even_with_a_build_output() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let source = env.archive(
        "rootsite",
        &[
            ("package.json", r#"{"name":"rootsite","version":"1.0.0","scripts":{"build":"node build.js"}}"#),
            (
                "build.js",
                "require('fs').mkdirSync('dist', { recursive: true });\n\
                 require('fs').writeFileSync('dist/index.html', '<h1>from-dist</h1>');\n\
                 require('fs').writeFileSync('dist/app.js', 'console.log(1)');\n",
            ),
            ("index.html", "<h1>from-root</h1><script src=\"/dist/app.js\"></script>"),
        ],
    );
    let mut req = env.request("rootsite", ServiceType::StaticSite, source);
    req.publish_dir = Some(".".into());
    let (res, logs) = env.build(&req).await;
    if let Err(e) = res {
        panic!("build failed: {e}\n{}", logs.join("\n"));
    }
    assert!(logs.iter().any(|l| l.contains("Publishing the root directory")), "{}", logs.join("\n"));
    let port = env.run_http(&req.image_tag);
    let (code, body) = http_get(port, "/");
    assert_eq!(code, 200);
    assert!(body.contains("from-root"), "{body}");
    assert_eq!(http_get(port, "/dist/app.js").0, 200);
    // Ferry's own files are not published.
    assert_eq!(http_get(port, "/Dockerfile.ferry").0, 404);
}

/// Regression: `npm install` of a package that does not exist was
/// summarized as "404 tarball, folder, http url, or git url.".
#[tokio::test]
async fn npm_install_failure_names_the_missing_package() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let missing = format!("ferryweb-no-such-package-{}", ids::random_secret(8).to_ascii_lowercase());
    let package_json = format!(
        r#"{{"name":"npm404","version":"1.0.0","dependencies":{{"{missing}":"1.0.0"}},"scripts":{{"start":"node -e 0"}}}}"#
    );
    let source = env.archive("npm404", &[("package.json", &package_json)]);
    let req = env.request("npm404", ServiceType::BackgroundWorker, source);
    let (res, logs) = env.build(&req).await;
    let msg = match res {
        Err(Error::Build(m)) => m,
        other => panic!("expected a build failure, got {other:?}\n{}", logs.join("\n")),
    };
    if msg.starts_with("request to ") {
        eprintln!("skipped: the npm registry is unreachable from Docker builds ({msg})");
        return;
    }
    assert_eq!(msg, format!("'{missing}@1.0.0' is not in this registry (npm E404)"), "{}", logs.join("\n"));
    assert!(logs.iter().any(|l| l == &format!("==> Build failed: {msg}")));
}

/// Runs only inside [`sigkilled_server_leaves_no_docker_build_running`]'s
/// child process: a server whose build is stuck in a slow `RUN` step.
#[tokio::test]
async fn orphan_helper() {
    let Some(spec) = std::env::var_os("FERRYWEB_ORPHAN_HELPER") else { return };
    let spec = spec.to_string_lossy().into_owned();
    let (root, tag) = spec.split_once('|').expect("root|tag");
    let root = PathBuf::from(root);
    let archive = root.join("slow.tar.gz");
    let mut req = request(
        "srv-slow",
        "dep-slow",
        "slow",
        ServiceType::BackgroundWorker,
        BuildSource::Archive { path: archive },
        tag,
    );
    req.clear_cache = true;
    let _ = new_builder(&root).build(&req, &LogSink::noop(), &CancellationToken::new()).await;
}

/// `ps -o comm= -p <pid>` (empty when the process is gone).
fn comm_of(pid: u32) -> String {
    let out = Command::new("ps").args(["-o", "comm=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Regression: after `kill -9` of ferryd, its `docker build` kept running
/// (re-parented to init) and the next server did not reap it.
#[test]
fn sigkilled_server_leaves_no_docker_build_running() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let src = env.root.join("slow-src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("Dockerfile"), "FROM busybox:stable\nRUN echo slow-step && sleep 300\n").unwrap();
    tar_gz(&src, None, &env.root.join("slow.tar.gz"));
    let tag = env.naming.image_tag("slow", "dep-slow");
    env.tags.lock().unwrap().push(tag.clone());

    let mut server = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "orphan_helper", "--nocapture", "--test-threads=1"])
        .env("FERRYWEB_ORPHAN_HELPER", format!("{}|{tag}", env.root.display()))
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Wait for the recorded `docker build`, give it time to start the step.
    let records = env.root.join("data/builds/dep-slow/children");
    let start = Instant::now();
    let pid: u32 = loop {
        let found = fs::read_dir(&records)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .find(|e| fs::read_to_string(e.path()).is_ok_and(|r| r.trim_end().ends_with(" docker")));
        if let Some(pid) = found.and_then(|e| e.file_name().to_str().and_then(|n| n.parse().ok())) {
            break pid;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "docker build was never recorded");
        std::thread::sleep(Duration::from_millis(50));
    };
    std::thread::sleep(Duration::from_secs(3));
    server.kill().unwrap();
    server.wait().unwrap();

    let reaped = new_builder(&env.root).reap_orphans();
    if cfg!(target_os = "linux") {
        // PR_SET_PDEATHSIG already killed the docker CLI.
        assert!(reaped <= 1, "{reaped}");
    } else {
        assert_eq!(reaped, 1, "the orphaned docker build was not reaped");
    }
    let start = Instant::now();
    while !comm_of(pid).is_empty() && !comm_of(pid).contains("defunct") {
        assert!(start.elapsed() < Duration::from_secs(10), "docker build {pid} ({}) still running", comm_of(pid));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(fs::read_dir(&records).unwrap().next().is_none(), "records left behind");
    // The killed build never produced its image.
    std::thread::sleep(Duration::from_secs(2));
    let inspect = Command::new("docker").args(["image", "inspect", &tag]).output().unwrap();
    assert!(!inspect.status.success(), "the killed build still produced {tag}");
}
