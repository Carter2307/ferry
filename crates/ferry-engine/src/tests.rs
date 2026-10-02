//! Engine tests that need no Docker daemon: the Docker client points at an
//! unreachable address, so anything reaching Docker fails fast and the
//! queue / validation / failure paths can be checked deterministically.

use std::sync::Arc;
use std::time::Duration;

use ferry_build::Builder;
use ferry_core::{
    CheckOutcome, Config, Datastore, DatastoreKind, Deploy, DeployRequest, DeploySource, DeployStatus, DeployTrigger,
    Domain, DomainSource, DomainStatus, Engine, Error, GitConnection, GitProvider, JobRun, JobStatus, JobTrigger,
    LogLine, Service, ServiceType, Store,
};
use ferry_docker::Docker;
use ferry_proxy::{Resolution, RouteTable};
use futures::StreamExt;

use crate::FerryEngine;
use crate::logs::LogKind;

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Arc<FerryEngine>,
    store: Store,
    routes: RouteTable,
}

async fn fixture(build_concurrency: usize) -> Fixture {
    fixture_with(build_concurrency, "docker").await
}

/// `docker_bin` is the CLI the builder runs `docker build` with.
async fn fixture_with(build_concurrency: usize, docker_bin: &str) -> Fixture {
    fixture_config(docker_bin, |c| c.build_concurrency = build_concurrency).await
}

async fn fixture_config(docker_bin: &str, tweak: impl FnOnce(&mut Config)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut config =
        Config { data_dir: dir.path().to_path_buf(), name_prefix: "ferrytest-unit".into(), ..Config::default() };
    tweak(&mut config);
    let config = Arc::new(config);
    let store = Store::open(&config.db_path()).await.unwrap();
    // Port 9 (discard) on loopback: connections are refused immediately.
    let bollard = bollard::Docker::connect_with_http("http://127.0.0.1:9", 2, bollard::API_DEFAULT_VERSION).unwrap();
    let docker = Docker::from_bollard(bollard);
    let builder = Builder::new(config.builds_dir(), config.repos_dir(), docker_bin.into());
    let routes = RouteTable::new();
    let engine = FerryEngine::new(config, store.clone(), docker, builder, routes.clone());
    Fixture { _dir: dir, engine, store, routes }
}

async fn service(store: &Store, name: &str, kind: ServiceType, f: impl FnOnce(&mut Service)) -> Service {
    let mut s = Service::new(name, kind);
    f(&mut s);
    store.create_service(&s).await.unwrap();
    s
}

async fn wait_status(store: &Store, id: &str, pred: impl Fn(DeployStatus) -> bool) -> Deploy {
    for _ in 0..200 {
        let d = store.require_deploy(id).await.unwrap();
        if pred(d.status) {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("deploy {id} never reached the expected status");
}

async fn log_lines(engine: &FerryEngine, id: &str) -> Vec<String> {
    let stream = engine.deploy_logs(id, true).await.unwrap();
    let lines: Vec<LogLine> = tokio::time::timeout(Duration::from_secs(10), stream.collect()).await.unwrap();
    lines.into_iter().map(|l| l.line).collect()
}

#[tokio::test]
async fn deploy_validation_errors() {
    let f = fixture(2).await;
    let e = &f.engine;
    assert!(matches!(e.deploy("nope", DeployRequest::new(DeployTrigger::Manual)).await, Err(Error::NotFound(_))));

    let suspended = service(&f.store, "sus", ServiceType::WebService, |s| {
        s.image = Some("nginx:alpine".into());
        s.suspended = true;
    })
    .await;
    assert!(matches!(
        e.deploy(&suspended.id, DeployRequest::new(DeployTrigger::Manual)).await,
        Err(Error::Conflict(_))
    ));

    let upload = service(&f.store, "up", ServiceType::WebService, |_| {}).await;
    match e.deploy(&upload.id, DeployRequest::new(DeployTrigger::Manual)).await {
        Err(Error::Invalid(m)) => assert!(m.contains("ferry up"), "{m}"),
        other => panic!("expected Invalid, got {other:?}"),
    }

    let image = service(&f.store, "img", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let mut req = DeployRequest::new(DeployTrigger::Manual);
    req.commit = Some("abc123".into());
    assert!(matches!(e.deploy(&image.id, req).await, Err(Error::Invalid(_))));

    // Restart / rollback need something to reuse.
    assert!(matches!(e.restart(&image.id, DeployTrigger::Restart).await, Err(Error::Conflict(_))));
    // Only deploys that went live can be rolled back to.
    let mut failed =
        Deploy::new(&image.id, DeployTrigger::Manual, DeploySource::Image { image: "nginx:alpine".into() });
    failed.status = DeployStatus::DeployFailed;
    failed.image = Some("ferrytest-unit/img:dep-x".into());
    f.store.create_deploy(&failed).await.unwrap();
    match e.rollback(&image.id, &failed.id).await {
        Err(Error::Invalid(m)) => assert!(m.contains("never went live"), "{m}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    let other = service(&f.store, "other", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let foreign = Deploy::new(&other.id, DeployTrigger::Manual, DeploySource::Image { image: "nginx:alpine".into() });
    f.store.create_deploy(&foreign).await.unwrap();
    assert!(matches!(e.rollback(&image.id, &foreign.id).await, Err(Error::Invalid(_))));
    let imageless = Deploy::new(&image.id, DeployTrigger::Manual, DeploySource::Image { image: "nginx:alpine".into() });
    f.store.create_deploy(&imageless).await.unwrap();
    match e.rollback(&image.id, &imageless.id).await {
        Err(Error::Invalid(m)) => assert!(m.contains("no image"), "{m}"),
        other => panic!("expected Invalid, got {other:?}"),
    }
    assert!(matches!(e.deploy_logs("dep-missing", false).await, Err(Error::NotFound(_))));
    assert!(matches!(e.job_logs("job-missing", false).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn failing_pull_marks_build_failed_with_logs() {
    let f = fixture(2).await;
    let svc = service(&f.store, "web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Create)).await.unwrap();
    assert_eq!(d.status, DeployStatus::Queued);
    // Following from the start sees every line and ends with the deploy.
    let lines = log_lines(&f.engine, &d.id).await;
    let d = wait_status(&f.store, &d.id, |s| s.is_terminal()).await;
    assert_eq!(d.status, DeployStatus::BuildFailed, "{d:?}");
    assert!(d.error.is_some() && d.finished_at.is_some() && d.started_at.is_some());
    assert!(lines.first().is_some_and(|l| l.starts_with("==> Starting deploy")), "{lines:?}");
    // Named after the failed stage, like the status (build_failed).
    assert!(lines.last().is_some_and(|l| l.starts_with("==> Build failed:")), "{lines:?}");
    // Nothing went live; the service is still deployable.
    assert!(f.store.require_service(&svc.id).await.unwrap().live_deploy_id.is_none());
    // Terminal deploys cannot be canceled.
    assert!(matches!(f.engine.cancel_deploy(&d.id).await, Err(Error::Conflict(_))));
    // A replayed log equals the followed one.
    let replay: Vec<String> =
        f.engine.deploy_logs(&d.id, false).await.unwrap().map(|l| l.line).collect::<Vec<_>>().await;
    assert_eq!(replay, lines);
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=Ferry Test", "-c", "user.email=test@ferry.invalid"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn failed_builds_record_the_checked_out_commit() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: git is not installed");
        return;
    }
    // The checkout works; `docker build` can't even start.
    let f = fixture_with(2, "/nonexistent/ferry-test-docker").await;
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("Dockerfile"), "FROM busybox:stable\nRUN exit 3\n").unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "broken build"]);
    let sha = git(repo.path(), &["rev-parse", "HEAD"]);
    let svc = service(&f.store, "echo", ServiceType::WebService, |s| {
        s.repo_url = Some(repo.path().display().to_string());
        s.branch = "main".into();
    })
    .await;
    let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let lines = log_lines(&f.engine, &d.id).await;
    let d = wait_status(&f.store, &d.id, |s| s.is_terminal()).await;
    assert_eq!(d.status, DeployStatus::BuildFailed, "{d:?}\n{lines:?}");
    // The failed deploy says which commit broke the build.
    assert_eq!(d.commit_sha.as_deref(), Some(sha.as_str()), "{lines:?}");
    assert_eq!(d.commit_message.as_deref(), Some("broken build"));
    assert!(lines.iter().any(|l| l.starts_with("==> Build failed")), "{lines:?}");
}

/// A dumb-HTTP git server for the bare repository `repo` (at `/repo.git`)
/// that demands the Basic credentials `user:password`. Returns its port and
/// whether an authorized request was served.
async fn private_git_server(
    repo: std::path::PathBuf,
    user: &str,
    password: &str,
) -> (u16, Arc<std::sync::atomic::AtomicBool>) {
    use base64::Engine as _;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}")));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let authorized = Arc::new(AtomicBool::new(false));
    let seen = authorized.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (repo, expected, seen) = (repo.clone(), expected.clone(), seen.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let req = String::from_utf8_lossy(&buf).into_owned();
                let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/");
                let auth_ok = req.lines().any(|l| {
                    l.split_once(':')
                        .is_some_and(|(k, v)| k.eq_ignore_ascii_case("authorization") && v.trim() == expected)
                });
                let file = path.strip_prefix("/repo.git/").and_then(|rel| std::fs::read(repo.join(rel)).ok());
                let head = |status: &str, extra: &str, len: usize| {
                    format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {len}\r\nConnection: close\r\n\r\n")
                        .into_bytes()
                };
                let response = match (auth_ok, file) {
                    (false, _) => head("401 Unauthorized", "WWW-Authenticate: Basic realm=\"git\"\r\n", 0),
                    (true, Some(body)) => {
                        seen.store(true, Ordering::SeqCst);
                        let mut r = head("200 OK", "Content-Type: application/octet-stream\r\n", body.len());
                        r.extend_from_slice(&body);
                        r
                    }
                    (true, None) => head("404 Not Found", "", 0),
                };
                let _ = sock.write_all(&response).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, authorized)
}

#[tokio::test]
async fn git_connections_authenticate_clones_on_their_own_host() {
    use std::sync::atomic::Ordering;
    if std::process::Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: git is not installed");
        return;
    }
    const TOKEN: &str = "ghp_s3cr3tT0kenOfTheConnection";
    // The checkout is real; `docker build` can't even start, so every deploy
    // ends `build_failed` — with its commit recorded when the clone worked.
    let f = fixture_with(2, "/nonexistent/ferry-test-docker").await;
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("Dockerfile"), "FROM busybox:stable\n").unwrap();
    git(src.path(), &["init", "-q", "-b", "main"]);
    git(src.path(), &["add", "-A"]);
    git(src.path(), &["commit", "-q", "-m", "private app"]);
    let sha = git(src.path(), &["rev-parse", "HEAD"]);
    let served = tempfile::tempdir().unwrap();
    git(served.path(), &["clone", "-q", "--bare", &src.path().display().to_string(), "repo.git"]);
    git(&served.path().join("repo.git"), &["update-server-info"]);
    let (port, authorized) = private_git_server(served.path().join("repo.git"), "x-access-token", TOKEN).await;
    let repo_url = format!("http://127.0.0.1:{port}/repo.git");

    // Services don't name a connection: the server's connections decide.
    let deploy = async |name: &str| {
        let svc = service(&f.store, name, ServiceType::WebService, |s| s.repo_url = Some(repo_url.clone())).await;
        let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
        let lines = log_lines(&f.engine, &d.id).await;
        let d = wait_status(&f.store, &d.id, |s| s.is_terminal()).await;
        assert_eq!(d.status, DeployStatus::BuildFailed, "{d:?}\n{lines:?}");
        // No token ever reaches the deploy log, the deploy's error or its source.
        let stored = format!("{lines:?} {:?} {:?}", d.error, d.source);
        assert!(!stored.contains("ghp_") && !stored.contains("glpat-"), "{stored}");
        (d, lines)
    };
    let has = |lines: &[String], prefix: &str| lines.iter().any(|l| l.starts_with(prefix));
    let instance = format!("http://127.0.0.1:{port}");

    // Without a connection the private repository can't be cloned.
    let (d, lines) = deploy("anonymous").await;
    assert_eq!(d.commit_sha, None, "{lines:?}");
    assert!(has(&lines, "==> Hint: if the repository is private, connect its GitHub or GitLab account"), "{lines:?}");
    assert!(!authorized.load(Ordering::SeqCst));

    // A connection for another host is never used for this one.
    let elsewhere = GitConnection::new(GitProvider::Gitlab, "https://gitlab.com", "me", "glpat-0therS3cret");
    f.store.create_git_connection(&elsewhere).await.unwrap();
    let (d, lines) = deploy("elsewhere").await;
    assert_eq!(d.commit_sha, None, "{lines:?}");
    assert!(!has(&lines, "==> Cloning with") && !authorized.load(Ordering::SeqCst), "{lines:?}");

    // A connection whose token the remote refuses says what to do about it.
    let stale = GitConnection::new(GitProvider::Github, instance.clone(), "ghost", "ghp_expired0");
    f.store.create_git_connection(&stale).await.unwrap();
    let (d, lines) = deploy("stale").await;
    assert_eq!(d.commit_sha, None, "{lines:?}");
    assert!(lines.iter().any(|l| l == "==> Cloning with the GitHub account 'ghost'"), "{lines:?}");
    assert!(has(&lines, "==> Hint: the token of the GitHub account 'ghost' may have expired"), "{lines:?}");
    assert!(!authorized.load(Ordering::SeqCst));
    f.store.delete_git_connection(&stale.id).await.unwrap();

    // A connection that can't produce a token (here: an app whose key is
    // unreadable) is said, and the clone goes on without it.
    let app = ferry_core::GithubApp {
        id: 1,
        slug: "ferry-test".into(),
        url: format!("{instance}/apps/ferry-test"),
        private_key: "not a key".into(),
        webhook_secret: None,
    };
    let mut broken = GitConnection::github_app(instance.clone(), "repo.git", app, "Iv1", "s");
    broken.installation = Some(ferry_core::GithubInstallation { id: 1, url: None, repository_selection: None });
    f.store.create_git_connection(&broken).await.unwrap();
    let (d, lines) = deploy("broken").await;
    assert_eq!(d.commit_sha, None, "{lines:?}");
    let warning = "==> Warning: the GitHub account 'repo.git' can't be used (the GitHub App's private key can't be \
                   read): cloning without it";
    assert!(lines.iter().any(|l| l == warning), "{lines:?}");
    assert!(has(&lines, "==> Hint: the GitHub App of the GitHub account 'repo.git' may not be allowed"), "{lines:?}");
    f.store.delete_git_connection(&broken.id).await.unwrap();

    // The connection of the repository's own host: its token clones it.
    let connection = GitConnection::new(GitProvider::Github, instance.clone(), "octocat", TOKEN);
    f.store.create_git_connection(&connection).await.unwrap();
    let (d, lines) = deploy("private").await;
    assert!(authorized.load(Ordering::SeqCst), "{lines:?}");
    assert_eq!(d.commit_sha.as_deref(), Some(sha.as_str()), "{lines:?}");
    assert_eq!(d.commit_message.as_deref(), Some("private app"));
    assert!(lines.iter().any(|l| l == "==> Cloning with the GitHub account 'octocat'"), "{lines:?}");
    assert!(lines.iter().any(|l| l == &format!("==> Cloning from {repo_url} (branch main)")), "{lines:?}");
    assert!(!has(&lines, "==> Hint"), "{lines:?}");
    assert_eq!(d.source, DeploySource::Git { repo_url: repo_url.clone(), branch: "main".into(), commit: None });

    // Once the connection is deleted, the same service clones without it again.
    f.store.delete_git_connection(&connection.id).await.unwrap();
    let svc = f.store.require_service("private").await.unwrap();
    let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let lines = log_lines(&f.engine, &d.id).await;
    assert!(!has(&lines, "==> Cloning with"), "{lines:?}");
}

#[tokio::test]
async fn newer_deploys_supersede_queued_ones_and_queued_ones_can_be_canceled() {
    let f = fixture(1).await;
    let svc = service(&f.store, "api", ServiceType::WebService, |s| {
        s.repo_url = Some("/nonexistent/repo".into());
    })
    .await;
    // Occupy the only build slot: git deploys wait in the queue.
    let slot = f.engine.inner.build_slots.clone().acquire_owned().await.unwrap();
    let d1 = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let d2 = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let d3 = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Webhook)).await.unwrap();

    for (old, by) in [(&d1, &d2), (&d2, &d3)] {
        let d = f.store.require_deploy(&old.id).await.unwrap();
        assert_eq!(d.status, DeployStatus::Canceled);
        assert_eq!(d.error.as_deref(), Some(format!("superseded by {}", by.id).as_str()));
        let lines = log_lines(&f.engine, &old.id).await;
        assert!(lines.iter().any(|l| l.contains("superseded by")), "{lines:?}");
    }
    assert_eq!(f.store.require_deploy(&d3.id).await.unwrap().status, DeployStatus::Queued);

    // Following a queued deploy works; canceling it ends the stream.
    let follower = tokio::spawn(f.engine.deploy_logs(&d3.id, true).await.unwrap().collect::<Vec<_>>());
    let canceled = f.engine.cancel_deploy(&d3.id).await.unwrap();
    assert_eq!(canceled.status, DeployStatus::Canceled);
    assert_eq!(canceled.error.as_deref(), Some("canceled by user"));
    let lines = tokio::time::timeout(Duration::from_secs(5), follower).await.unwrap().unwrap();
    assert!(lines.iter().any(|l| l.line.contains("Waiting for a free build slot")), "{lines:?}");
    assert!(lines.last().is_some_and(|l| l.line.contains("canceled")), "{lines:?}");
    drop(slot);

    // The worker went idle; a later deploy is still picked up and runs
    // (and fails: the repository does not exist).
    let d4 = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let d4 = wait_status(&f.store, &d4.id, |s| s.is_terminal()).await;
    assert_eq!(d4.status, DeployStatus::BuildFailed, "{d4:?}");
    for _ in 0..100 {
        if f.engine.inner.with_rt(|rt| rt.workers.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(f.engine.inner.with_rt(|rt| rt.workers.is_empty() && rt.deploys.is_empty()));
}

#[tokio::test]
async fn jobs_and_scale_validation() {
    let f = fixture(2).await;
    let e = &f.engine;
    let web = service(&f.store, "web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    assert!(matches!(e.run_job(&web.id, None, JobTrigger::Manual).await, Err(Error::Invalid(_))));
    assert!(matches!(e.run_job(&web.id, Some("  ".into()), JobTrigger::Manual).await, Err(Error::Invalid(_))));
    assert!(matches!(e.run_job(&web.id, Some("ls".into()), JobTrigger::Manual).await, Err(Error::Conflict(_))));

    assert!(matches!(e.scale(&web.id, 0).await, Err(Error::Invalid(_))));
    assert!(matches!(e.scale(&web.id, 51).await, Err(Error::Invalid(_))));
    let disk = service(&f.store, "disk", ServiceType::WebService, |s| s.disk_mount_path = Some("/data".into())).await;
    assert!(matches!(e.scale(&disk.id, 2).await, Err(Error::Invalid(_))));

    let cron = service(&f.store, "cron", ServiceType::CronJob, |s| s.schedule = Some("* * * * *".into())).await;
    e.scale(&cron.id, 3).await.unwrap();
    assert_eq!(f.store.require_service(&cron.id).await.unwrap().instances, 3);
    assert!(matches!(e.scale("missing", 1).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn boot_recovery_fails_interrupted_work() {
    let f = fixture(2).await;
    let svc = service(&f.store, "web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let mut ids = Vec::new();
    for status in [DeployStatus::Queued, DeployStatus::Building, DeployStatus::Deploying, DeployStatus::Live] {
        let mut d = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "nginx:alpine".into() });
        d.status = status;
        f.store.create_deploy(&d).await.unwrap();
        ids.push(d.id);
    }
    let mut job = JobRun::new(&svc.id, JobTrigger::Manual, Some("ls".into()));
    job.status = JobStatus::Running;
    f.store.create_job_run(&job).await.unwrap();

    crate::deploy::recover_interrupted(&f.engine.inner).await.unwrap();
    let status = |i: usize| {
        let store = f.store.clone();
        let id = ids[i].clone();
        async move { store.require_deploy(&id).await.unwrap() }
    };
    assert_eq!(status(0).await.status, DeployStatus::BuildFailed);
    assert_eq!(status(1).await.status, DeployStatus::BuildFailed);
    let d = status(2).await;
    assert_eq!(d.status, DeployStatus::DeployFailed);
    assert_eq!(d.error.as_deref(), Some("interrupted by server restart"));
    assert_eq!(status(3).await.status, DeployStatus::Live);
    let j = f.store.require_job_run(&job.id).await.unwrap();
    assert_eq!(j.status, JobStatus::Failed);
    assert!(j.finished_at.is_some());
    let lines = log_lines(&f.engine, &ids[2]).await;
    assert_eq!(lines, vec!["==> Deploy failed: interrupted by server restart"]);
    assert!(!f.engine.inner.logs.is_open(LogKind::Job, &job.id));
}

#[tokio::test]
async fn refresh_routes_and_suspend_route_states() {
    let f = fixture(2).await;
    let svc = service(&f.store, "web", ServiceType::WebService, |s| {
        s.image = Some("nginx:alpine".into());
        s.custom_domains = vec!["app.example.com".into()];
    })
    .await;
    // Busy service (e.g. a deploy holds it): only hosts are refreshed.
    let upstream: std::net::SocketAddr = "127.0.0.1:4242".parse().unwrap();
    f.routes.set_service_routes(&svc.id, &["web.localhost".into()], vec![upstream]);
    {
        let _busy = f.engine.inner.service_locks.try_lock(&svc.id).unwrap();
        f.engine.refresh_routes(&svc.id).await.unwrap();
    }
    assert_eq!(f.routes.resolve("app.example.com"), Resolution::Upstream(upstream));
    assert_eq!(f.routes.resolve("web.localhost:8080"), Resolution::Upstream(upstream));

    // Private services have no routes.
    let private = service(&f.store, "priv", ServiceType::PrivateService, |_| {}).await;
    f.engine.refresh_routes(&private.id).await.unwrap();
    assert!(f.routes.snapshot().iter().all(|r| r.service_id != private.id));

    // Suspend with Docker unreachable: the proxy answers 503 right away and
    // the store says suspended, even though the containers can't be listed.
    assert!(f.engine.suspend(&svc.id).await.is_err());
    assert_eq!(f.routes.resolve("web.localhost"), Resolution::Suspended);
    assert!(f.store.require_service(&svc.id).await.unwrap().suspended);
    assert!(matches!(
        f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await,
        Err(Error::Conflict(_))
    ));
}

#[tokio::test]
async fn datastore_provisioning_failure_is_recorded() {
    let f = fixture(2).await;
    let ds = Datastore::new("db", DatastoreKind::Postgres);
    f.store.create_datastore(&ds).await.unwrap();
    f.engine.provision_datastore(&ds.id).await.unwrap();
    for _ in 0..200 {
        let d = f.store.require_datastore(&ds.id).await.unwrap();
        if d.status == ferry_core::DatastoreStatus::Failed {
            assert!(d.error.is_some());
            assert!(d.host_port.is_some(), "the host port is allocated before the container starts");
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("provisioning never failed");
}

#[tokio::test]
async fn datastore_limits_need_an_existing_datastore() {
    let f = fixture(2).await;
    assert!(matches!(f.engine.update_datastore_limits("dbs-missing").await, Err(Error::NotFound(_))));
    // Docker is unreachable: the container can't be looked up.
    let ds = Datastore::new("db", DatastoreKind::Redis);
    f.store.create_datastore(&ds).await.unwrap();
    assert!(f.engine.update_datastore_limits(&ds.id).await.is_err());
}

#[tokio::test]
async fn deploys_fail_early_when_the_disk_is_nearly_full() {
    if !cfg!(unix) {
        return;
    }
    // Nobody has a pebibyte free.
    let f = fixture_config("docker", |c| c.min_free_disk_mb = 1 << 30).await;
    let svc = service(&f.store, "web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let lines = log_lines(&f.engine, &d.id).await;
    let d = wait_status(&f.store, &d.id, |s| s.is_terminal()).await;
    assert_eq!(d.status, DeployStatus::BuildFailed, "{lines:?}");
    let error = d.error.unwrap_or_default();
    assert!(error.starts_with("not enough free disk space on "), "{error}");
    assert!(error.contains("at least 1048576 GiB") && error.contains("`ferryd --min-free-disk`"), "{error}");
    // Before anything is pulled or built.
    assert!(!lines.iter().any(|l| l.contains("Pulling")), "{lines:?}");
    assert!(lines.last().is_some_and(|l| l.starts_with("==> Build failed: not enough free disk space")), "{lines:?}");
}

#[tokio::test]
async fn start_twice_is_a_conflict_and_needs_docker() {
    let f = fixture(2).await;
    let shutdown = ferry_core::CancellationToken::new();
    // Docker is unreachable: the network can't be created.
    assert!(f.engine.start(shutdown.clone()).await.is_err());
    assert!(matches!(f.engine.start(shutdown.clone()).await, Err(Error::Conflict(_))));
    shutdown.cancel();
}

#[tokio::test]
async fn restarts_fold_into_queued_deploys_and_follow_the_live_deploy() {
    let f = fixture(1).await;
    let svc =
        service(&f.store, "api", ServiceType::WebService, |s| s.repo_url = Some("/nonexistent/repo".into())).await;
    let mut live1 = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
    live1.status = DeployStatus::Deactivated;
    live1.image = Some("ferrytest-unit/api:one".into());
    f.store.create_deploy(&live1).await.unwrap();
    let mut live2 = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
    live2.status = DeployStatus::Live;
    live2.image = Some("ferrytest-unit/api:two".into());
    f.store.create_deploy(&live2).await.unwrap();
    f.store.set_live_deploy(&svc.id, Some(&live2.id)).await.unwrap();

    // A restart while a build is queued returns that queued deploy.
    let slot = f.engine.inner.build_slots.clone().acquire_owned().await.unwrap();
    let queued = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Webhook)).await.unwrap();
    let restart = f.engine.restart(&svc.id, DeployTrigger::EnvChange).await.unwrap();
    assert_eq!(restart.id, queued.id);
    assert_eq!(f.store.require_deploy(&queued.id).await.unwrap().status, DeployStatus::Queued);
    f.engine.cancel_deploy(&queued.id).await.unwrap();
    drop(slot);

    // A restart queued against deploy one runs against what is live when it starts.
    let stale = Deploy::new(
        &svc.id,
        DeployTrigger::Restart,
        DeploySource::Reuse { image: "ferrytest-unit/api:one".into(), from_deploy: Some(live1.id.clone()) },
    );
    f.store.create_deploy(&stale).await.unwrap();
    let running = f.store.set_deploy_status(&stale.id, DeployStatus::Building, None).await.unwrap();
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let active = crate::state::ActiveDeploy::new(&svc.id, ferry_core::CancellationToken::new(), done_rx);
    let log = f.engine.inner.logs.open(LogKind::Deploy, &stale.id);
    crate::pipeline::run(f.engine.inner.clone(), running, active, log.clone(), Default::default(), None).await;
    log.finish().await;
    let d = f.store.require_deploy(&stale.id).await.unwrap();
    assert_eq!(
        d.source,
        DeploySource::Reuse { image: "ferrytest-unit/api:two".into(), from_deploy: Some(live2.id.clone()) }
    );
    // (Docker is unreachable, so it fails right after.)
    assert_eq!(d.status, DeployStatus::BuildFailed);
    let lines = log_lines(&f.engine, &stale.id).await;
    assert!(lines.iter().any(|l| l.contains("restarting") && l.contains(&live2.id)), "{lines:?}");

    // Rollbacks keep their explicit target.
    let rollback = Deploy::new(
        &svc.id,
        DeployTrigger::Rollback,
        DeploySource::Reuse { image: "ferrytest-unit/api:one".into(), from_deploy: Some(live1.id.clone()) },
    );
    f.store.create_deploy(&rollback).await.unwrap();
    let running = f.store.set_deploy_status(&rollback.id, DeployStatus::Building, None).await.unwrap();
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let active = crate::state::ActiveDeploy::new(&svc.id, ferry_core::CancellationToken::new(), done_rx);
    let log = f.engine.inner.logs.open(LogKind::Deploy, &rollback.id);
    crate::pipeline::run(f.engine.inner.clone(), running, active, log.clone(), Default::default(), None).await;
    log.finish().await;
    let d = f.store.require_deploy(&rollback.id).await.unwrap();
    assert!(matches!(d.source, DeploySource::Reuse { from_deploy: Some(ref from), .. } if *from == live1.id));
}

#[tokio::test]
async fn cancel_after_the_swap_is_refused_at_once() {
    let f = fixture(2).await;
    let svc = service(&f.store, "web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let mut d = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "nginx:alpine".into() });
    d.status = DeployStatus::Deploying;
    f.store.create_deploy(&d).await.unwrap();
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let active = crate::state::ActiveDeploy::new(&svc.id, ferry_core::CancellationToken::new(), done_rx);
    f.engine.inner.with_rt(|rt| rt.deploys.insert(d.id.clone(), active.clone()));

    // Traffic switched: canceling is a conflict right away (no waiting for
    // the old instances to drain), and the deploy is not canceled.
    assert!(active.commit_swap());
    let started = std::time::Instant::now();
    match f.engine.cancel_deploy(&d.id).await {
        Err(Error::Conflict(m)) => assert!(m.contains("too late"), "{m}"),
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!active.cancel.is_cancelled());
    // Once live (marked right after the swap), the same.
    f.store.set_deploy_status(&d.id, DeployStatus::Live, None).await.unwrap();
    assert!(matches!(f.engine.cancel_deploy(&d.id).await, Err(Error::Conflict(m)) if m.contains("too late")));

    // Before the swap a cancel wins, and the swap is then refused.
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let other = crate::state::ActiveDeploy::new(&svc.id, ferry_core::CancellationToken::new(), done_rx);
    assert!(other.request_cancel("canceled by user"));
    assert!(!other.commit_swap());
    assert_eq!(other.cancel_reason().as_deref(), Some("canceled by user"));
}

#[tokio::test]
async fn restart_queues_behind_a_first_deploy_in_progress() {
    let f = fixture(1).await;
    let svc =
        service(&f.store, "api", ServiceType::WebService, |s| s.repo_url = Some("/nonexistent/repo".into())).await;
    // A first deploy is being built: nothing is live yet.
    let mut first = Deploy::new(
        &svc.id,
        DeployTrigger::Create,
        DeploySource::Git { repo_url: "/nonexistent/repo".into(), branch: "main".into(), commit: None },
    );
    first.status = DeployStatus::Building;
    f.store.create_deploy(&first).await.unwrap();
    // The service's only worker is busy building it, so what is queued
    // behind it stays queued (`first` is just a row here: a stand-in worker
    // keeps a real one from claiming the restart, which needs no build slot).
    let busy = crate::state::Worker { generation: u64::MAX, wake: Arc::new(tokio::sync::Notify::new()), handle: None };
    f.engine.inner.with_rt(|rt| rt.workers.insert(svc.id.clone(), busy));

    let restart = f.engine.restart(&svc.id, DeployTrigger::EnvChange).await.unwrap();
    assert_ne!(restart.id, first.id);
    assert_eq!(restart.status, DeployStatus::Queued);
    assert_eq!(restart.trigger, DeployTrigger::EnvChange);
    assert!(matches!(restart.source, DeploySource::Reuse { from_deploy: Some(ref from), .. } if *from == first.id));
    // A second env change folds into the queued restart.
    let again = f.engine.restart(&svc.id, DeployTrigger::EnvChange).await.unwrap();
    assert_eq!(again.id, restart.id);
    // The first deploy is not superseded (it is not queued).
    assert_eq!(f.store.require_deploy(&first.id).await.unwrap().status, DeployStatus::Building);

    // The first deploy failed and the worker moves on to the restart, which
    // has nothing to restart and says so.
    f.store.set_deploy_status(&first.id, DeployStatus::BuildFailed, Some("boom")).await.unwrap();
    f.engine.inner.with_rt(|rt| rt.workers.remove(&svc.id));
    crate::deploy::ensure_worker(&f.engine.inner, &svc.id);
    let done = wait_status(&f.store, &restart.id, |s| s.is_terminal()).await;
    assert_eq!(done.status, DeployStatus::BuildFailed);
    assert!(done.error.as_deref().is_some_and(|e| e.contains("no live deploy")), "{done:?}");
    // Without any deploy at all, a restart is still a conflict.
    let idle = service(&f.store, "idle", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    assert!(matches!(f.engine.restart(&idle.id, DeployTrigger::Restart).await, Err(Error::Conflict(_))));
}

#[tokio::test]
async fn a_public_service_gets_its_route_when_first_deployed() {
    let f = fixture(2).await;
    let svc = service(&f.store, "fresh", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    assert_eq!(f.routes.resolve("fresh.localhost"), Resolution::NotFound);
    let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Create)).await.unwrap();
    // 503 "no healthy instances yet" rather than 404 "unknown host".
    assert_eq!(f.routes.resolve("fresh.localhost"), Resolution::NoUpstreams);
    wait_status(&f.store, &d.id, |s| s.is_terminal()).await;
    // Workers have no route.
    let worker =
        service(&f.store, "bg", ServiceType::BackgroundWorker, |s| s.image = Some("busybox:stable".into())).await;
    f.engine.deploy(&worker.id, DeployRequest::new(DeployTrigger::Create)).await.unwrap();
    assert!(f.routes.snapshot().iter().all(|r| r.service_id != worker.id));
}

#[tokio::test]
async fn shutdown_fails_queued_deploys_and_ends_their_followers() {
    let f = fixture(1).await;
    let svc = service(&f.store, "api", ServiceType::WebService, |s| {
        s.repo_url = Some("/nonexistent/repo".into());
    })
    .await;
    // The only build slot is taken: the deploy stays queued.
    let slot = f.engine.inner.build_slots.clone().acquire_owned().await.unwrap();
    let d = f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await.unwrap();
    let follower = tokio::spawn(f.engine.deploy_logs(&d.id, true).await.unwrap().collect::<Vec<_>>());
    f.engine.inner.started.store(true, std::sync::atomic::Ordering::SeqCst);
    f.engine.inner.spawn(crate::deploy::fail_queued_on_shutdown(f.engine.inner.clone()));
    f.engine.inner.shutdown.cancel();

    // The follower's stream ends (instead of waiting forever), the deploy is
    // failed, and the engine reports it has stopped.
    let lines = tokio::time::timeout(Duration::from_secs(10), follower).await.unwrap().unwrap();
    assert!(lines.last().is_some_and(|l| l.line.contains("interrupted by server shutdown")), "{lines:?}");
    let d = f.store.require_deploy(&d.id).await.unwrap();
    assert_eq!(d.status, DeployStatus::BuildFailed);
    assert_eq!(d.error.as_deref(), Some("interrupted by server shutdown"));
    tokio::time::timeout(Duration::from_secs(10), f.engine.stopped()).await.expect("the engine stops");
    // Nothing new is accepted while shutting down.
    assert!(matches!(
        f.engine.deploy(&svc.id, DeployRequest::new(DeployTrigger::Manual)).await,
        Err(Error::Conflict(m)) if m.contains("shutting down")
    ));
    drop(slot);
}

#[tokio::test]
async fn stopped_resolves_at_once_when_never_started() {
    let f = fixture(1).await;
    tokio::time::timeout(Duration::from_secs(1), f.engine.stopped()).await.expect("nothing to wait for");
}

/// Wait until a job has finished and its task is done (log finished,
/// retention applied).
async fn wait_job_done(f: &Fixture, id: &str) -> JobRun {
    for _ in 0..400 {
        let job = f.store.get_job_run(id).await.unwrap();
        let running = f.engine.inner.with_rt(|rt| rt.jobs.contains_key(id));
        match job {
            Some(j) if j.status.is_terminal() && !running => return j,
            None if !running => panic!("job {id} was deleted"),
            _ => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
    panic!("job {id} never finished");
}

/// A cron service with a live deploy (Docker is unreachable, so its runs
/// fail right away, which is all retention needs).
async fn live_cron(f: &Fixture, name: &str) -> Service {
    let svc = service(&f.store, name, ServiceType::CronJob, |s| {
        s.image = Some("busybox:stable".into());
        s.schedule = Some("0 0 * * *".into());
        s.start_command = Some("echo hi".into());
    })
    .await;
    let mut live = Deploy::new(&svc.id, DeployTrigger::Create, DeploySource::Image { image: "busybox:stable".into() });
    live.status = DeployStatus::Live;
    live.image = Some(format!("ferrytest-unit/{name}:{}", live.id));
    f.store.create_deploy(&live).await.unwrap();
    f.store.set_live_deploy(&svc.id, Some(&live.id)).await.unwrap();
    f.store.require_service(&svc.id).await.unwrap()
}

#[tokio::test]
async fn finished_job_runs_beyond_the_retention_limit_are_deleted_with_their_logs() {
    let f = fixture_config("docker", |c| c.keep_job_runs = 2).await;
    let svc = live_cron(&f, "tick").await;
    let log_file = |id: &str| f.engine.inner.logs.path(LogKind::Job, id).unwrap();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let job = f.engine.run_job(&svc.id, None, JobTrigger::Manual).await.unwrap();
        let done = wait_job_done(&f, &job.id).await;
        assert_eq!(done.status, JobStatus::Failed, "{done:?}");
        ids.push(job.id);
    }
    let kept: Vec<String> = f.store.list_job_runs(&svc.id, 100).await.unwrap().into_iter().map(|j| j.id).collect();
    assert_eq!(kept, vec![ids[3].clone(), ids[2].clone()], "the newest 2 finished runs are kept");
    for id in &ids[..2] {
        assert!(!log_file(id).exists(), "the log of pruned run {id} is deleted");
    }
    for id in &ids[2..] {
        assert!(log_file(id).exists(), "the log of kept run {id} is still there");
    }
    // Pending / running runs never count against the limit.
    let mut running = JobRun::new(&svc.id, JobTrigger::Manual, None);
    running.status = JobStatus::Running;
    f.store.create_job_run(&running).await.unwrap();
    let job = f.engine.run_job(&svc.id, None, JobTrigger::Manual).await.unwrap();
    wait_job_done(&f, &job.id).await;
    assert!(f.store.get_job_run(&running.id).await.unwrap().is_some());
    assert_eq!(f.store.list_job_runs(&svc.id, 100).await.unwrap().len(), 3);
}

#[tokio::test]
async fn cancel_job_conflicts_once_finished_and_recovers_orphaned_rows() {
    let f = fixture(2).await;
    let svc = live_cron(&f, "tick").await;
    assert!(matches!(f.engine.cancel_job("job-missing").await, Err(Error::NotFound(_))));

    // Finished: Conflict, nothing changes.
    let job = f.engine.run_job(&svc.id, None, JobTrigger::Manual).await.unwrap();
    let done = wait_job_done(&f, &job.id).await;
    match f.engine.cancel_job(&job.id).await {
        Err(Error::Conflict(m)) => assert!(m.contains("already finished"), "{m}"),
        other => panic!("expected Conflict, got {other:?}"),
    }
    assert_eq!(f.store.require_job_run(&job.id).await.unwrap().status, done.status);

    // A run no task of this server owns (e.g. its result could not be
    // recorded): canceled right away, its log finished.
    let mut orphan = JobRun::new(&svc.id, JobTrigger::Schedule, None);
    orphan.status = JobStatus::Running;
    orphan.started_at = Some(chrono::Utc::now());
    f.store.create_job_run(&orphan).await.unwrap();
    let canceled = f.engine.cancel_job(&orphan.id).await.unwrap();
    assert_eq!(canceled.status, JobStatus::Canceled);
    assert_eq!(canceled.error.as_deref(), Some("canceled by user"));
    assert!(canceled.finished_at.is_some());
    assert!(!f.engine.inner.logs.is_open(LogKind::Job, &orphan.id));
    let lines: Vec<String> =
        f.engine.job_logs(&orphan.id, true).await.unwrap().map(|l| l.line).collect::<Vec<_>>().await;
    assert_eq!(lines, vec!["==> Job canceled: canceled by user"]);
    assert!(matches!(f.engine.cancel_job(&orphan.id).await, Err(Error::Conflict(_))));

    // A run of a suspended service is refused without leaving anything registered.
    f.store.set_suspended(&svc.id, true).await.unwrap();
    assert!(matches!(f.engine.run_job(&svc.id, None, JobTrigger::Manual).await, Err(Error::Conflict(_))));
    assert!(f.engine.inner.with_rt(|rt| rt.jobs.is_empty()));
}

// ---------------------------------------------------------------------------
// domains (DESIGN.md §21)

use crate::domains::tests::{FakeNet, ip};

const HERE: &str = "203.0.113.10";

/// An engine whose network is `FakeNet`, with the base domain in the store.
async fn domains_fixture() -> (Fixture, Arc<FakeNet>) {
    let f = fixture(2).await;
    let net = Arc::new(FakeNet::default());
    *net.public.lock().unwrap() = vec![ip(HERE)];
    f.engine.inner.domains.set_network(net.clone());
    ferry_core::domains::init(&f.store, &f.engine.inner.config).await.unwrap();
    (f, net)
}

#[tokio::test]
async fn services_are_served_under_a_domain_once_it_reaches_the_server() {
    let (f, net) = domains_fixture().await;
    let config = f.engine.inner.config.clone();
    let svc = service(&f.store, "web", ServiceType::WebService, |s| s.image = Some("nginx:alpine".into())).await;
    let upstream: std::net::SocketAddr = "127.0.0.1:4242".parse().unwrap();
    f.routes.set_service_routes(&svc.id, &["web.localhost".into()], vec![upstream]);

    // Connected, and its DNS isn't there yet: pending, nothing served.
    let d = Domain::new("example.com", DomainSource::Connected);
    f.store.create_domain(&d).await.unwrap();
    f.engine.refresh_domains().await.unwrap();
    assert_eq!(f.routes.resolve("web.example.com"), Resolution::NotFound);
    let checked = f.engine.verify_domain(&d.id).await.unwrap();
    assert_eq!((checked.status, checked.checks[0].outcome), (DomainStatus::Pending, CheckOutcome::Failed));
    assert!(checked.checked_at.is_some() && checked.verified_at.is_none());
    assert_eq!(f.routes.resolve("web.example.com"), Resolution::NotFound);

    // The record exists and this server answers: served at once, with the
    // upstreams the service had, and the domain of its URL from now on (the
    // default one was a local name).
    net.point("example.com", &[HERE]);
    net.answer(HERE, Ok(config.domains.probe_id()));
    let verified = f.engine.verify_domain("example.com").await.unwrap();
    assert_eq!(verified.status, DomainStatus::Active);
    assert!(verified.is_default && verified.verified_at.is_some());
    assert_eq!(f.routes.resolve("web.example.com"), Resolution::Upstream(upstream));
    assert_eq!(f.routes.resolve("web.localhost"), Resolution::Upstream(upstream));
    assert_eq!(config.default_host("web"), "web.example.com");
    assert!(!f.store.require_domain("localhost").await.unwrap().is_default);

    // Its DNS breaks: flagged after three verifications, and still served.
    net.point("example.com", &["198.51.100.7"]);
    for expected in [DomainStatus::Active, DomainStatus::Active, DomainStatus::Misconfigured] {
        assert_eq!(f.engine.verify_domain(&d.id).await.unwrap().status, expected);
    }
    assert_eq!(f.routes.resolve("web.example.com"), Resolution::Upstream(upstream));
    net.point("example.com", &[HERE]);
    assert_eq!(f.engine.verify_domain(&d.id).await.unwrap().status, DomainStatus::Active);

    // Removed: its hostnames go, the base domain is the default again.
    f.store.delete_domain(&d.id).await.unwrap();
    f.engine.refresh_domains().await.unwrap();
    assert_eq!(f.routes.resolve("web.example.com"), Resolution::NotFound);
    assert_eq!(f.routes.resolve("web.localhost"), Resolution::Upstream(upstream));
    assert_eq!(config.default_host("web"), "web.localhost");
    assert!(matches!(f.engine.verify_domain(&d.id).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn a_second_domain_does_not_take_the_default_and_local_ones_need_no_dns() {
    let (f, net) = domains_fixture().await;
    let config = f.engine.inner.config.clone();
    net.answer(HERE, Ok(config.domains.probe_id()));
    for name in ["example.com", "example.org"] {
        net.point(name, &[HERE]);
        f.store.create_domain(&Domain::new(name, DomainSource::Connected)).await.unwrap();
        assert_eq!(f.engine.verify_domain(name).await.unwrap().status, DomainStatus::Active);
    }
    // The first one that worked replaced the local default; the second didn't.
    assert_eq!(config.primary_domain(), "example.com");
    assert_eq!(config.served_domains(), vec!["example.com", "localhost", "example.org"]);

    // A local name is served as soon as it is connected, and never verified.
    let local = Domain::new("dev.localhost", DomainSource::Connected);
    f.store.create_domain(&local).await.unwrap();
    f.engine.refresh_domains().await.unwrap();
    assert!(config.served_domains().contains(&"dev.localhost".to_string()));
    let asked = net.requests.lock().unwrap().len();
    let same = f.engine.verify_domain("dev.localhost").await.unwrap();
    assert_eq!((same.status, same.checked_at), (DomainStatus::Active, None));
    assert_eq!(net.requests.lock().unwrap().len(), asked);
}

#[tokio::test]
async fn the_public_address_is_the_configured_one_else_the_one_found() {
    let (f, net) = domains_fixture().await;
    assert_eq!(f.engine.public_addresses().await, vec![ip(HERE)]);
    // Found once, then remembered.
    *net.public.lock().unwrap() = vec![ip("198.51.100.7")];
    assert_eq!(f.engine.public_addresses().await, vec![ip(HERE)]);

    let configured = fixture_config("docker", |c| c.public_ips = vec![ip("192.0.2.44")]).await;
    configured.engine.inner.domains.set_network(net);
    assert_eq!(configured.engine.public_addresses().await, vec![ip("192.0.2.44")]);
}
