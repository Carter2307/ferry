//! Docker-gated regression tests for verified build findings: build-time
//! env stays out of image metadata, friendly build-step errors, static-site
//! dotfiles, rooted uploads, pyproject apps and non-Node static builds.
//! Run with `FERRY_E2E=1 cargo test -p ferry-build --test fix_regressions_e2e`;
//! without it every test returns early ("skipped").
//!
//! Images are tagged `ferryfix-build-<random>/<name>:<deploy>`; images and
//! containers are removed afterwards, even when a test fails.
#![cfg(unix)]

mod common;

use std::fs;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::*;
use ferry_build::{BuildOutput, BuildRequest, BuildSource, Builder};
use ferry_core::naming::Naming;
use ferry_core::{CancellationToken, Error, LogSink, Result, Runtime, ServiceType, ids};

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
    root: std::path::PathBuf,
    builder: Builder,
    tags: Mutex<Vec<String>>,
}

impl Env {
    fn new() -> Env {
        let prefix = format!("ferryfix-build-{}", ids::random_secret(8));
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let builder = Builder::new(root.join("data/builds"), root.join("data/repos"), "docker".to_string());
        Env { naming: Naming::new(&prefix), prefix, _dir: dir, root, builder, tags: Mutex::new(Vec::new()) }
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

    async fn build_ok(&self, req: &BuildRequest) -> Vec<String> {
        let (res, logs) = self.build(req).await;
        if let Err(e) = res {
            panic!("build failed: {e}\n{}", logs.join("\n"));
        }
        logs
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

/// Everything Docker records about an image: history and config.
fn image_metadata(tag: &str) -> String {
    let history = docker(&["history", "--no-trunc", "--format", "{{.CreatedBy}}", tag]);
    let inspect = docker(&["image", "inspect", tag]);
    format!("{history}\n{inspect}")
}

/// Status code of `GET path` on 127.0.0.1:port (retrying while the server starts).
fn http_status(port: u16, path: &str) -> u16 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let attempt = (|| -> std::io::Result<u16> {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port))?;
            s.set_read_timeout(Some(Duration::from_secs(5)))?;
            write!(s, "GET {path} HTTP/1.0\r\nHost: site.localhost\r\n\r\n")?;
            let mut buf = String::new();
            s.read_to_string(&mut buf)?;
            buf.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| std::io::Error::other(buf))
        })();
        match attempt {
            Ok(code) => return code,
            Err(e) if Instant::now() > deadline => panic!("GET {path} on port {port}: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn step_ran(logs: &[String], marker: &str) -> bool {
    logs.iter().any(|l| l.contains(marker) && !l.contains("RUN") && !l.contains("echo"))
}

const NODE_APP: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"envapp","version":"1.0.0","scripts":{"build":"node build.js","start":"node server.js"}}"#,
    ),
    (
        "build.js",
        "const s = process.env.API_SECRET || '';\n\
         if (!s.startsWith('hunter2-')) { console.error('error: API_SECRET is not visible to the build'); process.exit(1); }\n\
         console.log('built-with-env len=' + s.length + ' public=' + process.env.PUBLIC_NAME);\n",
    ),
    ("server.js", "require('http').createServer((q, r) => r.end('ok')).listen(process.env.PORT || 10000);\n"),
];

/// Generated Dockerfiles: env values reach the install/build steps but never
/// the image history or config, and the layer cache still follows them.
#[tokio::test]
async fn build_env_is_usable_but_not_recorded_in_the_image() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let secret = format!("hunter2-{}", ids::random_secret(16));
    let build = |value: &str| {
        let mut req = env.request("envapp", ServiceType::WebService, env.archive("envapp", NODE_APP));
        req.build_args = vec![("API_SECRET".into(), value.to_string()), ("PUBLIC_NAME".into(), "ferry".into())];
        req
    };

    let req = build(&secret);
    let logs = env.build_ok(&req).await;
    assert!(step_ran(&logs, "built-with-env len=24 public=ferry"), "{}", logs.join("\n"));
    let meta = image_metadata(&req.image_tag);
    eprintln!("image history:\n{}", docker(&["history", "--no-trunc", "--format", "{{.CreatedBy}}", &req.image_tag]));
    assert!(!meta.contains(&secret), "secret recorded in the image:\n{meta}");
    assert!(!meta.contains("API_SECRET="), "{meta}");
    assert!(!logs.iter().any(|l| l.contains("SecretsUsedInArgOrEnv")), "{}", logs.join("\n"));
    assert!(!logs.iter().any(|l| l.contains(&secret)), "secret in the build log");

    // Same values: the build step is cached.
    let req = build(&secret);
    let logs = env.build_ok(&req).await;
    assert!(!step_ran(&logs, "built-with-env"), "expected a cached build step:\n{}", logs.join("\n"));

    // A changed value re-runs it.
    let rotated = format!("hunter2-{}", ids::random_secret(16));
    let req = build(&rotated);
    let logs = env.build_ok(&req).await;
    assert!(step_ran(&logs, "built-with-env len=24"), "{}", logs.join("\n"));
    assert!(!image_metadata(&req.image_tag).contains(&rotated));
}

/// User Dockerfiles keep receiving their declared ARGs and can mount the
/// env as BuildKit secrets, which stay out of the history.
#[tokio::test]
async fn user_dockerfiles_get_build_args_and_secret_mounts() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let token = format!("tok-{}", ids::random_secret(16));
    let source = env.archive(
        "userdf",
        &[(
            "Dockerfile",
            "FROM busybox:stable\n\
             ARG GREETING\n\
             RUN test \"$GREETING\" = \"hello world\" && echo greeting-ok\n\
             RUN --mount=type=secret,id=API_TOKEN,env=API_TOKEN test \"${#API_TOKEN}\" -eq 20 && echo token-mounted\n",
        )],
    );
    let mut req = env.request("userdf", ServiceType::BackgroundWorker, source);
    req.build_args = vec![("GREETING".into(), "hello world".into()), ("API_TOKEN".into(), token.clone())];
    // Secret values are not part of BuildKit's cache key: run the steps.
    req.clear_cache = true;
    let logs = env.build_ok(&req).await;
    assert!(step_ran(&logs, "greeting-ok") && step_ran(&logs, "token-mounted"), "{}", logs.join("\n"));
    assert!(!image_metadata(&req.image_tag).contains(&token));
    assert!(!logs.iter().any(|l| l.contains(&token)));
}

/// A failing generated step reports its own error, not BuildKit's
/// `process "/bin/sh -c …" did not complete successfully`.
#[tokio::test]
async fn failed_publish_step_reports_the_step_error() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let source = env.archive("pub", &[("gen.sh", "mkdir -p output && echo hi > output/index.html\n")]);
    let mut req = env.request("pub", ServiceType::StaticSite, source);
    req.build_command = Some("sh gen.sh".into());
    req.publish_dir = Some("nope".into());
    let (res, logs) = env.build(&req).await;
    match res {
        Err(Error::Build(m)) => {
            assert_eq!(m, "publish directory 'nope' not found after the build", "{}", logs.join("\n"))
        }
        other => panic!("expected a build failure, got {other:?}\n{}", logs.join("\n")),
    }
    assert!(logs.iter().any(|l| l == "==> Build failed: publish directory 'nope' not found after the build"));
    // The full BuildKit message stays in the log.
    assert!(logs.iter().any(|l| l.contains("did not complete successfully")), "{}", logs.join("\n"));
}

/// `ferry up` of a directory holding only `public/` with `--publish-dir
/// public`; nginx never serves dotfiles but keeps /.well-known/ public.
#[tokio::test]
async fn static_site_serves_no_dotfiles_and_keeps_its_only_directory() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let source = env.archive(
        "dots",
        &[
            ("public/index.html", "<h1>home</h1>"),
            ("public/.gitignore", "secret.txt\n"),
            ("public/.env.example", "API_KEY=changeme\n"),
            ("public/assets/.hidden", "x"),
            ("public/.well-known/security.txt", "Contact: mailto:security@example.com\n"),
        ],
    );
    let mut req = env.request("dots", ServiceType::StaticSite, source);
    req.publish_dir = Some("public".into());
    let logs = env.build_ok(&req).await;
    assert!(!logs.iter().any(|l| l.contains("does not exist")), "{}", logs.join("\n"));
    let port = env.run_http(&req.image_tag);
    assert_eq!(http_status(port, "/"), 200);
    assert_eq!(http_status(port, "/.well-known/security.txt"), 200);
    for hidden in ["/.gitignore", "/.env.example", "/assets/.hidden", "/.well-knownx"] {
        assert_eq!(http_status(port, hidden), 404, "{hidden}");
    }
}

/// `uv init`-style apps (PEP 621, no build system, several top-level
/// modules) build without `pip install .`.
#[tokio::test]
async fn pyproject_app_without_build_system_builds() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let source = env.archive(
        "uvapp",
        &[
            (
                "pyproject.toml",
                "[project]\nname = \"app\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\ndependencies = []\n",
            ),
            ("main.py", "import db\nprint(db.VALUE)\n"),
            ("db.py", "VALUE = 'uv-app-ok'\n"),
        ],
    );
    let req = env.request("uvapp", ServiceType::BackgroundWorker, source);
    let logs = env.build_ok(&req).await;
    assert!(logs.iter().any(|l| l == "==> Detected Python runtime"), "{}", logs.join("\n"));
    let out = docker(&["run", "--rm", "--label", &format!("ferry.instance={}", env.prefix), &req.image_tag]);
    assert_eq!(out.trim(), "uv-app-ok");
}

/// A static site built by a Python tool gets a Python build stage.
#[tokio::test]
async fn python_static_site_builds_with_python() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let source = env.archive(
        "pysite",
        &[
            ("requirements.txt", "\n"),
            (
                "gen.py",
                "import pathlib\np = pathlib.Path('site')\np.mkdir()\n(p / 'index.html').write_text('<h1>py</h1>')\n",
            ),
        ],
    );
    let mut req = env.request("pysite", ServiceType::StaticSite, source);
    req.build_command = Some("python gen.py".into());
    req.clear_cache = true;
    let (res, logs) = env.build(&req).await;
    let out = res.unwrap_or_else(|e| panic!("build failed: {e}\n{}", logs.join("\n")));
    assert_eq!(out.runtime, Runtime::Static);
    assert!(logs.iter().any(|l| l.contains("Publishing ./site")), "{}", logs.join("\n"));
    let port = env.run_http(&req.image_tag);
    assert_eq!(http_status(port, "/"), 200);
}
