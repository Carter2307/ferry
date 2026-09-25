//! Docker-gated end-to-end builds of the example apps. Run with
//! `FERRY_E2E=1 cargo test -p ferry-build --test build_e2e`; without it every
//! test returns early ("skipped").
//!
//! Every image is tagged `ferrytest-build-<random>/<name>:<deploy>` and
//! removed afterwards, even when a test fails.
#![cfg(unix)]

mod common;

use std::fs;
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
        let prefix = format!("ferrytest-build-{}", ids::random_secret(8));
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let builder = Builder::new(root.join("data/builds"), root.join("data/repos"), "docker".to_string());
        Env { naming: Naming::new(&prefix), prefix, _dir: dir, root, builder, tags: Mutex::new(Vec::new()) }
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
}

impl Drop for Env {
    fn drop(&mut self) {
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
        if let Ok(out) = Command::new("docker")
            .args(["ps", "-aq", "--filter"])
            .arg(format!("label=ferry.instance={}", self.prefix))
            .output()
        {
            for id in String::from_utf8_lossy(&out.stdout).lines() {
                let _ = Command::new("docker").args(["rm", "-f", id]).output();
            }
        }
    }
}

fn image_exists(tag: &str) -> bool {
    Command::new("docker").args(["image", "inspect", tag]).output().is_ok_and(|o| o.status.success())
}

fn image_label(tag: &str, label: &str) -> String {
    let out = Command::new("docker")
        .args(["image", "inspect", "-f", &format!("{{{{index .Config.Labels \"{label}\"}}}}"), tag])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_source(dir: &std::path::Path, commit: Option<String>) -> BuildSource {
    BuildSource::Git { repo_url: dir.to_string_lossy().into_owned(), branch: "main".into(), commit }
}

#[tokio::test]
async fn static_site_from_git() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let (repo, head) = example_repo(&env.root, "static-site");
    let req = env.request("site", ServiceType::StaticSite, git_source(&repo, None));
    let (res, logs) = env.build(&req).await;
    let out = res.unwrap_or_else(|e| panic!("build failed: {e}\n{}", logs.join("\n")));
    assert_eq!(out.port_hint, Some(80));
    assert_eq!(out.runtime, Runtime::Static);
    assert_eq!(out.commit_sha.as_deref(), Some(head.as_str()));
    assert_eq!(out.commit_message.as_deref(), Some("initial static-site"));
    assert!(image_exists(&req.image_tag));
    assert_eq!(image_label(&req.image_tag, "ferry.instance"), env.prefix);
    assert!(logs.iter().any(|l| l == "==> Build successful 🎉"), "{logs:?}");
    // The generated nginx config is valid and the site files are in place.
    let check = Command::new("docker")
        .args(["run", "--rm", "--label"])
        .arg(format!("ferry.instance={}", env.prefix))
        .args([
            &req.image_tag,
            "sh",
            "-c",
            "nginx -t && test -f /usr/share/nginx/html/about.html && ! test -e /usr/share/nginx/html/Dockerfile.ferry",
        ])
        .output()
        .unwrap();
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    assert!(fs::read_dir(env.root.join("data/builds")).unwrap().next().is_none(), "scratch dir left behind");
}

#[tokio::test]
async fn static_site_with_node_build_step() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let src = env.root.join("spa");
    fs::create_dir_all(&src).unwrap();
    // A build script with no dependencies (no registry access needed).
    fs::write(
        src.join("package.json"),
        r#"{"name":"spa","private":true,"scripts":{"build":"mkdir -p dist/docs && echo built > dist/index.html && echo docs > dist/docs/index.html"}}"#,
    )
    .unwrap();
    fs::write(src.join("README.md"), "not served").unwrap();
    fs::write(src.join(".gitignore"), "dist\nnode_modules\n").unwrap();
    common::git(&src, &["init", "-q", "-b", "main"]);
    let head = commit_all(&src, "spa");
    let mut req = env.request("spa", ServiceType::StaticSite, git_source(&src, None));
    // No layer cache, so the publish step really runs (and --no-cache is exercised).
    req.clear_cache = true;
    let (res, logs) = env.build(&req).await;
    let out = res.unwrap_or_else(|e| panic!("build failed: {e}\n{}", logs.join("\n")));
    assert_eq!(out.runtime, Runtime::Static);
    assert_eq!(out.port_hint, Some(80));
    assert_eq!(out.commit_sha.as_deref(), Some(head.as_str()));
    assert!(logs.iter().any(|l| l.contains("Publishing ./dist")), "{}", logs.join("\n"));
    let check = Command::new("docker")
        .args(["run", "--rm", "--label"])
        .arg(format!("ferry.instance={}", env.prefix))
        .args([
            &req.image_tag,
            "sh",
            "-c",
            "nginx -t && grep -q built /usr/share/nginx/html/index.html && test -f /usr/share/nginx/html/docs/index.html \
             && ! test -e /usr/share/nginx/html/README.md",
        ])
        .output()
        .unwrap();
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
}

#[tokio::test]
async fn node_and_python_from_git() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    for (example_name, runtime, cmd_check) in [
        ("node-hello", Runtime::Node, "test -f /app/server.js && node --version"),
        ("python-hello", Runtime::Python, "test -f /app/app.py && python --version"),
    ] {
        let (repo, head) = example_repo(&env.root, example_name);
        let mut req = env.request(example_name, ServiceType::WebService, git_source(&repo, None));
        req.build_args = vec![("SOME_SETTING".into(), "value".into())];
        let (res, logs) = env.build(&req).await;
        let out = res.unwrap_or_else(|e| panic!("{example_name} build failed: {e}\n{}", logs.join("\n")));
        if std::env::var("FERRY_E2E_VERBOSE").is_ok() {
            eprintln!("{example_name} build log:\n{}", logs.join("\n"));
        }
        assert_eq!(out.runtime, runtime);
        assert_eq!(out.port_hint, None);
        assert_eq!(out.commit_sha.as_deref(), Some(head.as_str()));
        assert!(image_exists(&req.image_tag), "{example_name}");
        let check = Command::new("docker")
            .args(["run", "--rm", "--label"])
            .arg(format!("ferry.instance={}", env.prefix))
            .args([&req.image_tag, "sh", "-c", cmd_check])
            .output()
            .unwrap();
        assert!(check.status.success(), "{example_name}: {}", String::from_utf8_lossy(&check.stderr));
    }
}

#[tokio::test]
async fn docker_runtime_older_commit_and_failing_dockerfile() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let (repo, good) = example_repo(&env.root, "docker-echo");
    let dockerfile = fs::read_to_string(repo.join("Dockerfile")).unwrap();
    fs::write(repo.join("Dockerfile"), format!("{dockerfile}RUN echo about to fail && exit 3\n")).unwrap();
    let bad = commit_all(&repo, "break the build");

    // HEAD is broken → Error::Build with a concise reason.
    let req = env.request("echo", ServiceType::WebService, git_source(&repo, None));
    let (res, logs) = env.build(&req).await;
    match res {
        Err(Error::Build(msg)) => {
            assert!(msg.contains("exit code: 3"), "unexpected reason: {msg}");
            assert!(logs.iter().any(|l| l.contains("about to fail")), "docker output not streamed");
        }
        other => panic!("expected a build failure, got {other:?}"),
    }
    assert!(!image_exists(&req.image_tag));

    // The older, good commit still builds.
    let req = env.request("echo", ServiceType::WebService, git_source(&repo, Some(good.clone())));
    let (res, logs) = env.build(&req).await;
    let out = res.unwrap_or_else(|e| panic!("build failed: {e}\n{}", logs.join("\n")));
    assert_eq!(out.runtime, Runtime::Docker);
    assert_eq!(out.commit_sha.as_deref(), Some(good.as_str()));
    assert_ne!(good, bad);
    assert!(logs.iter().any(|l| l == "==> Using Dockerfile at ./Dockerfile"), "{logs:?}");
    assert!(image_exists(&req.image_tag));
}

#[tokio::test]
async fn archive_source_and_build_args() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    // python-hello uploaded as `ferry up` would (here with a top-level dir).
    let archive = env.root.join("python-hello.tar.gz");
    tar_gz(&example("python-hello"), Some("python-hello"), &archive);
    let req = env.request("py", ServiceType::WebService, BuildSource::Archive { path: archive });
    let (res, logs) = env.build(&req).await;
    let out = res.unwrap_or_else(|e| panic!("build failed: {e}\n{}", logs.join("\n")));
    assert_eq!(out.runtime, Runtime::Python);
    assert_eq!(out.commit_sha, None);
    assert!(image_exists(&req.image_tag));

    // Build-arg values reach the build through the environment.
    let src = env.root.join("args");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("Dockerfile"),
        "FROM busybox:stable\nARG GREETING\nARG API_TOKEN\nRUN test \"$GREETING\" = \"hello world\" && test -n \"$API_TOKEN\"\n",
    )
    .unwrap();
    let archive = env.root.join("args.tar.gz");
    tar_gz(&src, None, &archive);
    let mut req = env.request("args", ServiceType::BackgroundWorker, BuildSource::Archive { path: archive });
    req.build_args = vec![("GREETING".into(), "hello world".into()), ("API_TOKEN".into(), "tok-e2e-secret".into())];
    req.clear_cache = true;
    let (res, logs) = env.build(&req).await;
    res.unwrap_or_else(|e| panic!("build failed: {e}\n{}", logs.join("\n")));
    assert!(image_exists(&req.image_tag));
    assert!(!logs.iter().any(|l| l.contains("tok-e2e-secret")), "secret leaked into logs");
}

#[tokio::test]
async fn cancellation_stops_docker_build() {
    if !e2e_enabled() {
        return;
    }
    let env = Env::new();
    let src = env.root.join("slow");
    fs::create_dir_all(&src).unwrap();
    let marker = ids::random_secret(8);
    fs::write(src.join("Dockerfile"), format!("FROM busybox:stable\nRUN echo slow-{marker} && sleep 120\n")).unwrap();
    let archive = env.root.join("slow.tar.gz");
    tar_gz(&src, None, &archive);
    let req = env.request("slow", ServiceType::BackgroundWorker, BuildSource::Archive { path: archive });

    let (logs, mut rx) = LogSink::channel();
    let cancel = CancellationToken::new();
    let watcher = {
        let cancel = cancel.clone();
        let marker = marker.clone();
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(l) = rx.recv().await {
                let hit = l.line.contains(&format!("slow-{marker}")) && !l.line.contains("RUN");
                seen.push(l.line);
                if hit {
                    cancel.cancel();
                }
            }
            seen
        })
    };
    let started = Instant::now();
    let res = tokio::time::timeout(Duration::from_secs(120), env.builder.build(&req, &logs, &cancel))
        .await
        .expect("cancel did not stop the build");
    drop(logs);
    let seen = watcher.await.unwrap();
    eprintln!("cancellation build log:\n{}", seen.join("\n"));
    assert!(matches!(res, Err(Error::Canceled)), "{res:?}\n{}", seen.join("\n"));
    assert!(started.elapsed() < Duration::from_secs(90), "took {:?}", started.elapsed());
    assert!(!image_exists(&req.image_tag));
    assert!(fs::read_dir(env.root.join("data/builds")).unwrap().next().is_none());
}
