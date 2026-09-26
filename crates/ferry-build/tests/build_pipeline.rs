//! End-to-end tests of `Builder::build` without a Docker daemon: a fake
//! `docker` CLI records what it is given (argv, environment, Dockerfile,
//! context), so fetch → detect → generate → build → cleanup is exercised
//! for real, including cancellation and failure reporting.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use ferry_build::{BuildSource, Builder};
use ferry_core::{CancellationToken, Error, LogSink, Runtime, ServiceType};

enum Mode {
    Succeed,
    Fail,
    Hang,
}

struct Fake {
    _root: tempfile::TempDir,
    root: PathBuf,
    out: PathBuf,
    builder: Builder,
}

impl Fake {
    fn new(mode: Mode) -> Fake {
        let root = tempfile::tempdir().unwrap();
        let rootp = root.path().to_path_buf();
        let out = rootp.join("out");
        fs::create_dir_all(&out).unwrap();
        let tail = match mode {
            // A step printing a secret it was given (as BuildKit would).
            Mode::Succeed => "echo \"#9 exporting to image\"\necho \"token is $FERRY_BUILD_SECRET_0\"\nexit 0\n",
            Mode::Fail => {
                "echo '#5 ERROR: process \"/bin/sh -c exit 3\" did not complete successfully' >&2\n\
                 echo 'ERROR: failed to build: failed to solve: boom happened' >&2\nexit 1\n"
            }
            Mode::Hang => "echo started\nsleep 60\n",
        };
        let script = format!(
            "#!/bin/sh\nout='{}'\nprintf '%s\\n' \"$@\" > \"$out/argv\"\nenv > \"$out/env\"\n\
             dockerfile=''\nctx=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    -f) dockerfile=\"$2\"; shift 2;;\n    \
             -t|--label|--build-arg|--secret) shift 2;;\n    *) ctx=\"$1\"; shift;;\n  esac\ndone\n\
             cp \"$dockerfile\" \"$out/Dockerfile\"\nls -A \"$ctx\" > \"$out/context\"\n\
             echo '#1 [internal] load build definition from Dockerfile' >&2\n{tail}",
            out.display()
        );
        let bin = rootp.join("fake-docker");
        fs::write(&bin, script).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let builder =
            Builder::new(rootp.join("data/builds"), rootp.join("data/repos"), bin.to_string_lossy().into_owned());
        Fake { _root: root, root: rootp, out, builder }
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.out.join(name)).unwrap_or_default()
    }

    fn builds_dir_is_empty(&self) -> bool {
        let dir = self.root.join("data/builds");
        !dir.exists() || fs::read_dir(dir).unwrap().next().is_none()
    }
}

#[tokio::test]
async fn git_node_build_end_to_end() {
    let fake = Fake::new(Mode::Succeed);
    let (repo, head) = example_repo(&fake.root, "node-hello");
    let mut req = request(
        "srv-node",
        "dep-node1",
        "web",
        ServiceType::WebService,
        BuildSource::Git { repo_url: repo.to_string_lossy().into_owned(), branch: "main".into(), commit: None },
        "ferrytest/web:dep-node1",
    );
    req.build_args = vec![
        ("API_TOKEN".into(), "tok-very-secret-123".into()),
        ("PATH".into(), "/evil".into()),
        ("NODE_ENV".into(), "production".into()),
    ];
    req.labels = BTreeMap::from([("ferry.managed".into(), "true".into()), ("ferry.deploy".into(), "dep-node1".into())]);
    req.clear_cache = true;

    let (logs, mut rx) = LogSink::channel();
    let out = fake.builder.build(&req, &logs, &CancellationToken::new()).await.unwrap();
    assert_eq!(out.image, "ferrytest/web:dep-node1");
    assert_eq!(out.commit_sha.as_deref(), Some(head.as_str()));
    assert_eq!(out.commit_message.as_deref(), Some("initial node-hello"));
    assert_eq!(out.runtime, Runtime::Node);
    assert_eq!(out.port_hint, None);

    let argv: Vec<String> = fake.read("argv").lines().map(str::to_string).collect();
    assert_eq!(&argv[..3], &["build", "--progress=plain", "--load"]);
    assert!(argv.windows(2).any(|w| w == ["-t", "ferrytest/web:dep-node1"]), "{argv:?}");
    assert!(argv.windows(2).any(|w| w == ["--label", "ferry.deploy=dep-node1"]), "{argv:?}");
    // Generated Dockerfile: the env goes in as BuildKit secrets (values in
    // the CLI's environment), never as build args.
    assert!(argv.windows(2).any(|w| w == ["--secret", "id=API_TOKEN,env=FERRY_BUILD_SECRET_0"]), "{argv:?}");
    assert!(argv.windows(2).any(|w| w == ["--secret", "id=NODE_ENV,env=FERRY_BUILD_SECRET_1"]), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "API_TOKEN" || a == "NODE_ENV"), "env passed as build args: {argv:?}");
    let digest = argv.iter().find_map(|a| a.strip_prefix("FERRY_BUILD_ENV_DIGEST=")).expect("env digest build arg");
    assert!(digest.len() == 32 && digest.chars().all(|c| c.is_ascii_hexdigit()), "{digest}");
    assert!(!argv.iter().any(|a| a.contains("PATH")), "reserved build arg passed: {argv:?}");
    assert!(argv.contains(&"--no-cache".to_string()));
    assert!(!argv.iter().any(|a| a.contains("tok-very-secret")), "secret in argv: {argv:?}");
    let env = fake.read("env");
    assert!(env.lines().any(|l| l == "FERRY_BUILD_SECRET_0=tok-very-secret-123"), "{env}");
    assert!(!env.lines().any(|l| l.starts_with("API_TOKEN=")), "user var configures the docker CLI: {env}");
    assert!(env.lines().any(|l| l == "DOCKER_BUILDKIT=1"), "{env}");
    assert!(!env.lines().any(|l| l == "PATH=/evil"), "{env}");
    assert!(fake.root.join("data/repos/.build-env-key").is_file(), "digest key not persisted");

    let dockerfile = fake.read("Dockerfile");
    assert!(dockerfile.contains("FROM node:22-alpine"), "{dockerfile}");
    assert!(!dockerfile.contains("ARG API_TOKEN") && !dockerfile.contains("ARG NODE_ENV"), "{dockerfile}");
    assert!(dockerfile.contains("--mount=type=secret,id=API_TOKEN,env=API_TOKEN"), "{dockerfile}");
    assert!(dockerfile.contains("ARG FERRY_BUILD_ENV_DIGEST"), "{dockerfile}");
    assert!(dockerfile.contains(r#"CMD ["npm","start"]"#), "{dockerfile}");
    let context = fake.read("context");
    for f in ["Dockerfile.ferry", ".dockerignore", "package.json", "server.js"] {
        assert!(context.lines().any(|l| l == f), "{f} missing from context: {context}");
    }
    assert!(!context.lines().any(|l| l == ".git"), "{context}");

    let lines = drain(&mut rx);
    let joined = lines.join("\n");
    assert!(joined.contains(&format!("==> Cloning from {} (branch main)", repo.display())), "{joined}");
    assert!(joined.contains(&format!("==> Checked out {}: initial node-hello", &head[..7])), "{joined}");
    assert!(joined.contains("==> Detected Node.js runtime"), "{joined}");
    assert!(joined.contains("==> Generated Dockerfile.ferry"), "{joined}");
    assert!(joined.contains("#9 exporting to image"), "docker output not streamed: {joined}");
    assert!(joined.contains("token is ***"), "secret not masked: {joined}");
    assert!(!joined.contains("tok-very-secret"), "{joined}");
    assert!(joined.ends_with("==> Build successful 🎉"), "{joined}");

    assert!(fake.builds_dir_is_empty(), "scratch dir left behind");
    assert!(fake.root.join("data/repos/srv-node").is_dir(), "git cache missing");

    // Rebuild at an explicit (older) commit after a new push.
    fs::write(repo.join("extra.txt"), "x").unwrap();
    let newer = commit_all(&repo, "second");
    assert_ne!(newer, head);
    req.deploy_id = "dep-node2".into();
    req.source = BuildSource::Git {
        repo_url: repo.to_string_lossy().into_owned(),
        branch: "main".into(),
        commit: Some(head.clone()),
    };
    let out = fake.builder.build(&req, &LogSink::noop(), &CancellationToken::new()).await.unwrap();
    assert_eq!(out.commit_sha.as_deref(), Some(head.as_str()));
    assert!(!fake.read("context").contains("extra.txt"));

    // Branch head resolution and cache removal.
    assert_eq!(fake.builder.resolve_branch_head(&repo.to_string_lossy(), "main").await.unwrap(), newer);
    let err = fake.builder.resolve_branch_head(&repo.to_string_lossy(), "nope").await.unwrap_err();
    assert!(err.to_string().contains("branch 'nope' not found"), "{err}");
    fake.builder.remove_repo_cache("srv-node").await.unwrap();
    assert!(!fake.root.join("data/repos/srv-node").exists());
    fake.builder.remove_repo_cache("srv-node").await.unwrap();
}

#[tokio::test]
async fn archive_static_site_with_top_level_dir() {
    let fake = Fake::new(Mode::Succeed);
    let archive = fake.root.join("upload.tar.gz");
    tar_gz(&example("static-site"), Some("static-site"), &archive);
    let req = request(
        "srv-static",
        "dep-static1",
        "site",
        ServiceType::StaticSite,
        BuildSource::Archive { path: archive },
        "ferrytest/site:dep-static1",
    );
    let (logs, mut rx) = LogSink::channel();
    let out = fake.builder.build(&req, &logs, &CancellationToken::new()).await.unwrap();
    assert_eq!(out.runtime, Runtime::Static);
    assert_eq!(out.port_hint, Some(80));
    assert_eq!(out.commit_sha, None);
    let context = fake.read("context");
    assert!(context.lines().any(|l| l == "index.html"), "top-level dir not stripped: {context}");
    let dockerfile = fake.read("Dockerfile");
    assert!(dockerfile.contains("FROM nginx:alpine"), "{dockerfile}");
    let lines = drain(&mut rx).join("\n");
    assert!(lines.contains("==> Extracting uploaded source archive"), "{lines}");
    assert!(lines.contains("==> Extracted 2 files"), "{lines}");
    assert!(fake.builds_dir_is_empty());
}

#[tokio::test]
async fn docker_runtime_with_root_dir_and_dockerfile_path() {
    let fake = Fake::new(Mode::Succeed);
    let src = fake.root.join("mono");
    copy_dir(&example("docker-echo"), &src.join("services/echo"));
    fs::create_dir_all(src.join("services/echo/deploy")).unwrap();
    fs::rename(src.join("services/echo/Dockerfile"), src.join("services/echo/deploy/Prod.Dockerfile")).unwrap();
    let archive = fake.root.join("mono.tar.gz");
    tar_gz(&src, None, &archive);
    let mut req = request(
        "srv-echo",
        "dep-echo1",
        "echo",
        ServiceType::PrivateService,
        BuildSource::Archive { path: archive },
        "ferrytest/echo:dep-echo1",
    );
    req.root_dir = Some("services/echo".into());
    req.dockerfile_path = Some("deploy/Prod.Dockerfile".into());
    req.build_args = vec![("GREETING".into(), "hello there".into())];
    let (logs, mut rx) = LogSink::channel();
    let out = fake.builder.build(&req, &logs, &CancellationToken::new()).await.unwrap();
    assert_eq!(out.runtime, Runtime::Docker);
    assert_eq!(out.port_hint, None);
    // User Dockerfiles: their ARGs keep working (`--build-arg KEY`, value
    // from the environment) and the env is also available as secrets.
    let argv: Vec<String> = fake.read("argv").lines().map(str::to_string).collect();
    assert!(argv.windows(2).any(|w| w == ["--build-arg", "GREETING"]), "{argv:?}");
    assert!(argv.windows(2).any(|w| w == ["--secret", "id=GREETING,env=FERRY_BUILD_SECRET_0"]), "{argv:?}");
    assert!(!argv.iter().any(|a| a.contains("FERRY_BUILD_ENV_DIGEST") || a.contains("hello there")), "{argv:?}");
    assert!(fake.read("env").lines().any(|l| l == "GREETING=hello there"));
    assert!(fake.read("Dockerfile").contains("FROM python:3.12-alpine"));
    let context = fake.read("context");
    assert!(context.lines().any(|l| l == "echo.py"), "{context}");
    assert!(!context.lines().any(|l| l == "Dockerfile.ferry" || l == ".dockerignore"), "{context}");
    let lines = drain(&mut rx).join("\n");
    assert!(lines.contains("==> Using root directory ./services/echo"), "{lines}");
    assert!(lines.contains("==> Using Dockerfile at ./deploy/Prod.Dockerfile"), "{lines}");

    // Missing Dockerfile with the docker runtime → clear build error.
    req.deploy_id = "dep-echo2".into();
    req.runtime = Runtime::Docker;
    req.dockerfile_path = None;
    let err = fake.builder.build(&req, &LogSink::noop(), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(&err, Error::Build(m) if m.contains("Dockerfile not found at ./Dockerfile")), "{err}");

    // root_dir escaping the source is rejected.
    req.root_dir = Some("../../etc".into());
    let err = fake.builder.build(&req, &LogSink::noop(), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(&err, Error::Build(m) if m.contains("outside")), "{err}");
    assert!(fake.builds_dir_is_empty());
}

#[tokio::test]
async fn failed_docker_build_reports_concise_reason() {
    let fake = Fake::new(Mode::Fail);
    let (repo, _) = example_repo(&fake.root, "python-hello");
    let req = request(
        "srv-py",
        "dep-py1",
        "py",
        ServiceType::WebService,
        BuildSource::Git { repo_url: repo.to_string_lossy().into_owned(), branch: "main".into(), commit: None },
        "ferrytest/py:dep-py1",
    );
    let (logs, mut rx) = LogSink::channel();
    let err = fake.builder.build(&req, &logs, &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(&err, Error::Build(m) if m == "boom happened"), "{err}");
    let dockerfile = fake.read("Dockerfile");
    assert!(
        dockerfile.contains("FROM python:3.12-slim") && dockerfile.contains(r#"CMD ["/bin/sh","-c","python app.py"]"#),
        "{dockerfile}"
    );
    let lines = drain(&mut rx);
    assert_eq!(lines.last().map(String::as_str), Some("==> Build failed: boom happened"));
    assert!(fake.builds_dir_is_empty());

    // A missing branch fails before docker runs.
    let mut req = req.clone();
    req.deploy_id = "dep-py2".into();
    req.source = BuildSource::Git { repo_url: repo.to_string_lossy().into_owned(), branch: "dev".into(), commit: None };
    let err = fake.builder.build(&req, &LogSink::noop(), &CancellationToken::new()).await.unwrap_err();
    let expected = format!("branch 'dev' not found in {} (the default branch is 'main')", repo.display());
    assert!(matches!(&err, Error::Build(m) if m == &expected), "{err}");
}

#[tokio::test]
async fn cancel_kills_docker_and_cleans_up() {
    let fake = Fake::new(Mode::Hang);
    let (repo, _) = example_repo(&fake.root, "static-site");
    let req = request(
        "srv-slow",
        "dep-slow1",
        "slow",
        ServiceType::StaticSite,
        BuildSource::Git { repo_url: repo.to_string_lossy().into_owned(), branch: "main".into(), commit: None },
        "ferrytest/slow:dep-slow1",
    );
    let (logs, mut rx) = LogSink::channel();
    let cancel = CancellationToken::new();
    let records_dir = fake.root.join("data/builds/dep-slow1/children");
    let watcher = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            // Cancel once the fake docker is running, after looking at the
            // records of the running children (for `reap_orphans`).
            let mut records = Vec::new();
            while let Some(l) = rx.recv().await {
                if l.line == "started" {
                    for e in fs::read_dir(&records_dir).unwrap() {
                        records.push(fs::read_to_string(e.unwrap().path()).unwrap());
                    }
                    cancel.cancel();
                    break;
                }
            }
            (rx, records)
        })
    };
    let started = Instant::now();
    let res = tokio::time::timeout(Duration::from_secs(30), fake.builder.build(&req, &logs, &cancel)).await.unwrap();
    assert!(matches!(res, Err(Error::Canceled)), "{res:?}");
    assert!(started.elapsed() < Duration::from_secs(15), "cancel took {:?}", started.elapsed());
    let (mut rx, records) = watcher.await.unwrap();
    assert!(drain(&mut rx).iter().any(|l| l == "==> Build canceled"));
    assert!(fake.builds_dir_is_empty());
    // The running docker CLI was recorded, outside the build context.
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(records[0].trim_end().ends_with(" fake-docker"), "{records:?}");
    let context = fake.read("context");
    assert!(context.lines().any(|l| l == "index.html"), "{context}");
    assert!(!context.lines().any(|l| l == "children" || l == "src"), "{context}");
    // Nothing is left to reap after a canceled build.
    assert_eq!(fake.builder.reap_orphans(), 0);
}

#[tokio::test]
async fn archive_path_traversal_is_rejected() {
    let fake = Fake::new(Mode::Succeed);
    // Hand-craft an archive with a "../" entry (tar::Builder refuses to).
    let mut header = tar::Header::new_gnu();
    let name = b"../escaped.txt";
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_size(1);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    let mut b = tar::Builder::new(Vec::new());
    b.append(&header, &b"x"[..]).unwrap();
    let data = b.into_inner().unwrap();
    let path = fake.root.join("evil.tar");
    fs::write(&path, data).unwrap();
    let req = request(
        "srv-evil",
        "dep-evil1",
        "evil",
        ServiceType::WebService,
        BuildSource::Archive { path },
        "ferrytest/evil:dep-evil1",
    );
    let err = fake.builder.build(&req, &LogSink::noop(), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(&err, Error::Build(m) if m.contains("path traversal")), "{err}");
    assert!(!fake.root.join("data/escaped.txt").exists());
    assert!(!fake.root.join("data/builds/escaped.txt").exists());
    assert!(fake.builds_dir_is_empty());
    assert!(!Path::new(&fake.out.join("argv")).exists(), "docker must not run");
}

/// Regression: `--runtime go|rust --build-cmd X` on a Python project built
/// "successfully" and crashed at start; the runtime check now also runs
/// with a build command, before docker is ever started.
#[tokio::test]
async fn explicit_runtime_is_checked_even_with_a_build_command() {
    let fake = Fake::new(Mode::Succeed);
    let src = fake.root.join("py");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("requirements.txt"), "flask\n").unwrap();
    fs::write(src.join("app.py"), "print('hi')\n").unwrap();
    let archive = fake.root.join("py.tar.gz");
    tar_gz(&src, None, &archive);
    for (runtime, manifest) in [(Runtime::Go, "go.mod"), (Runtime::Rust, "Cargo.toml")] {
        let mut req = request(
            "srv-py",
            &format!("dep-{runtime}"),
            "py",
            ServiceType::WebService,
            BuildSource::Archive { path: archive.clone() },
            "ferrytest/py:dep-x",
        );
        req.runtime = runtime;
        req.build_command = Some("make".into());
        req.start_command = Some("./app".into());
        let err = fake.builder.build(&req, &LogSink::noop(), &CancellationToken::new()).await.unwrap_err();
        let expected = format!(
            "runtime '{runtime}' was selected but the root directory has no {manifest} \
             (it looks like a Python project: set the runtime to 'python' or 'auto')"
        );
        assert!(matches!(&err, Error::Build(m) if m == &expected), "{err}");
    }
    assert!(fake.read("argv").is_empty(), "docker must not run");
    assert!(fake.builds_dir_is_empty());
}
