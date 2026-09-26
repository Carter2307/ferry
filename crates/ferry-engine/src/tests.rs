//! Engine tests that need no Docker daemon: the Docker client points at an
//! unreachable address, so anything reaching Docker fails fast and the
//! queue / validation / failure paths can be checked deterministically.

use std::sync::Arc;
use std::time::Duration;

use ferry_build::Builder;
use ferry_core::{
    Config, Datastore, DatastoreKind, Deploy, DeployRequest, DeploySource, DeployStatus, DeployTrigger, Engine, Error,
    JobRun, JobStatus, JobTrigger, LogLine, Service, ServiceType, Store,
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
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Config {
        data_dir: dir.path().to_path_buf(),
        name_prefix: "ferrytest-unit".into(),
        build_concurrency,
        ..Config::default()
    });
    let store = Store::open(&config.db_path()).await.unwrap();
    // Port 9 (discard) on loopback: connections are refused immediately.
    let bollard = bollard::Docker::connect_with_http("http://127.0.0.1:9", 2, bollard::API_DEFAULT_VERSION).unwrap();
    let docker = Docker::from_bollard(bollard);
    let builder = Builder::new(config.builds_dir(), config.repos_dir(), "docker".into());
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
    // A first deploy is being built (claimed by a worker): nothing is live yet.
    let mut first = Deploy::new(
        &svc.id,
        DeployTrigger::Create,
        DeploySource::Git { repo_url: "/nonexistent/repo".into(), branch: "main".into(), commit: None },
    );
    first.status = DeployStatus::Building;
    f.store.create_deploy(&first).await.unwrap();
    let slot = f.engine.inner.build_slots.clone().acquire_owned().await.unwrap();

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

    // The first deploy failed: the restart has nothing to restart and says so.
    f.store.set_deploy_status(&first.id, DeployStatus::BuildFailed, Some("boom")).await.unwrap();
    drop(slot);
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
