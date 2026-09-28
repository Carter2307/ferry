//! Docker-gated end-to-end tests. They only run with `FERRY_E2E=1` (otherwise
//! each test prints "skipped" and returns). Every Docker object they create is
//! prefixed `ferrytest-docker-<random>` (or `ferryfix-docker-<random>`),
//! labelled `ferry.instance=<prefix>`, and removed by a drop guard even when
//! an assertion fails.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use ferry_core::{Error, LogLine, LogSink, LogStreamKind, ids};
use ferry_docker::{
    ContainerSpec, ContainerState, Docker, LimitsUpdate, LogRotation, PortPublish, RestartPolicy, VolumeMount,
};
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BUSYBOX: &str = "busybox:stable";

fn e2e_enabled() -> bool {
    std::env::var("FERRY_E2E").is_ok_and(|v| v == "1")
}

macro_rules! require_e2e {
    () => {
        if !e2e_enabled() {
            eprintln!("skipped: set FERRY_E2E=1 to run Docker tests");
            return;
        }
    };
}

/// Removes everything a test created, even when it panics.
struct Cleanup {
    prefix: String,
    volumes: Vec<String>,
    networks: Vec<String>,
    images: Vec<String>,
}

impl Cleanup {
    fn new() -> Self {
        Self::with_prefix("ferrytest-docker")
    }

    fn with_prefix(base: &str) -> Self {
        Cleanup {
            prefix: format!("{base}-{}", ids::random_secret(8)),
            volumes: Vec::new(),
            networks: Vec::new(),
            images: Vec::new(),
        }
    }

    fn name(&self, suffix: &str) -> String {
        format!("{}-{suffix}", self.prefix)
    }

    fn labels(&self, extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut labels = BTreeMap::from([
            ("ferry.managed".to_string(), "true".to_string()),
            ("ferry.instance".to_string(), self.prefix.clone()),
        ]);
        labels.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        labels
    }

    /// A spec for a busybox container owned by this test.
    fn spec(&self, suffix: &str, script: &str) -> ContainerSpec {
        ContainerSpec {
            name: self.name(suffix),
            image: BUSYBOX.to_string(),
            cmd: Some(vec!["sh".into(), "-c".into(), script.into()]),
            labels: self.labels(&[]),
            ..Default::default()
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let docker = |args: &[&str]| Command::new("docker").args(args).output();
        let filter = format!("label=ferry.instance={}", self.prefix);
        if let Ok(out) = docker(&["ps", "-aq", "--filter", &filter]) {
            let ids: Vec<String> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from).collect();
            if !ids.is_empty() {
                let mut args = vec!["rm", "-f", "-v"];
                args.extend(ids.iter().map(String::as_str));
                let _ = docker(&args);
            }
        }
        for volume in &self.volumes {
            let _ = docker(&["volume", "rm", "-f", volume]);
        }
        for network in &self.networks {
            let _ = docker(&["network", "rm", network]);
        }
        for image in &self.images {
            let _ = docker(&["image", "rm", image]);
        }
    }
}

async fn connect() -> Docker {
    Docker::connect().await.expect("connect to Docker")
}

async fn collect(stream: ferry_core::LogStream) -> Vec<LogLine> {
    stream.collect().await
}

fn texts(lines: &[LogLine]) -> Vec<&str> {
    lines.iter().map(|l| l.line.as_str()).collect()
}

/// `GET <path>` over plain HTTP/1.0; returns the raw response.
async fn http_get(port: u16, path: &str) -> std::io::Result<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes()).await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Retry `http_get` until the response contains `needle` (the server may
/// still be starting).
async fn http_get_until(port: u16, path: &str, needle: &str) -> String {
    let mut last = String::new();
    for _ in 0..50 {
        match http_get(port, path).await {
            Ok(resp) if resp.contains(needle) => return resp,
            Ok(resp) => last = resp,
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("GET 127.0.0.1:{port}{path} never contained {needle:?}; last: {last}");
}

/// Poll the logs (non-follow) until `needle` shows up.
async fn logs_until(docker: &Docker, id: &str, needle: &str) -> Vec<LogLine> {
    for _ in 0..50 {
        let lines = collect(docker.logs(id, false, None)).await;
        if lines.iter().any(|l| l.line == needle) {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("container {id} never logged {needle:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn container_lifecycle() {
    require_e2e!();
    let mut cleanup = Cleanup::new();
    let docker = connect().await;
    assert!(!docker.version().await.unwrap().is_empty());

    // Network and volume: created once, idempotent afterwards.
    let network = cleanup.prefix.clone();
    cleanup.networks.push(network.clone());
    let volume = cleanup.name("data");
    cleanup.volumes.push(volume.clone());
    let labels = cleanup.labels(&[]);
    docker.ensure_network(&network, &labels).await.unwrap();
    docker.ensure_network(&network, &labels).await.unwrap();
    let inspected = docker.bollard().inspect_network(&network, None).await.unwrap();
    assert_eq!(inspected.name.as_deref(), Some(network.as_str()));
    assert_eq!(inspected.labels.unwrap_or_default().get("ferry.instance"), Some(&cleanup.prefix));
    docker.ensure_volume(&volume, &labels).await.unwrap();
    docker.ensure_volume(&volume, &labels).await.unwrap();
    let vol = docker.bollard().inspect_volume(&volume).await.unwrap();
    assert_eq!(vol.labels.get("ferry.instance"), Some(&cleanup.prefix));

    // Image already present: nothing is pulled.
    let (sink, mut pull_lines) = LogSink::channel();
    docker.ensure_image(BUSYBOX, &sink).await.unwrap();
    drop(sink);
    assert!(pull_lines.recv().await.is_none(), "ensure_image must not pull a present image");

    // A web server published on an ephemeral 127.0.0.1 port.
    let script = "mkdir -p /www && echo \"$GREETING\" > /www/index.html && echo persisted > /data/marker \
                  && echo started && echo oops >&2 && exec httpd -f -p 8080 -h /www";
    let spec = ContainerSpec {
        env: vec![("GREETING".into(), "hello-ferry".into())],
        labels: cleanup.labels(&[("ferry.role", "service"), ("test", "lifecycle")]),
        network: Some(network.clone()),
        network_aliases: vec!["web-alias".into()],
        publish: Some(PortPublish { container_port: 8080, host_ip: "127.0.0.1".into(), host_port: None }),
        volumes: vec![VolumeMount { volume: volume.clone(), target: "/data".into() }],
        restart_policy: RestartPolicy::UnlessStopped,
        memory_limit_bytes: Some(64 << 20),
        nano_cpus: Some(500_000_000),
        working_dir: Some("/www".into()),
        ..cleanup.spec("web", script)
    };
    let info = docker.run_container(&spec).await.unwrap();
    assert_eq!(info.name, spec.name);
    assert_eq!(info.image, BUSYBOX);
    assert_eq!(info.state, ContainerState::Running);
    assert_eq!(info.exit_code, None);
    assert!(info.started_at.is_some());
    assert_eq!(info.restart_count, Some(0));
    assert_eq!(info.labels.get("test").map(String::as_str), Some("lifecycle"));
    let host_port = info.host_port.expect("published host port");
    let id = info.id.clone();

    // Inspect by name and id agree; unknown containers are None.
    let by_name = docker.inspect_container(&spec.name).await.unwrap().unwrap();
    assert_eq!(by_name.id, id);
    assert_eq!(by_name.host_port, Some(host_port));
    assert!(docker.inspect_container(&cleanup.name("missing")).await.unwrap().is_none());
    assert!(matches!(docker.inspect_container("../etc").await, Err(Error::Invalid(_))));

    // Listing by labels.
    let listed =
        docker.list_containers(&[("ferry.instance", &cleanup.prefix), ("test", "lifecycle")], false).await.unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].name, spec.name);
    assert_eq!(listed[0].state, ContainerState::Running);
    assert_eq!(listed[0].host_port, Some(host_port));
    assert!(
        docker
            .list_containers(&[("ferry.instance", &cleanup.prefix), ("test", "other")], true)
            .await
            .unwrap()
            .is_empty()
    );

    // Reachable through the published port...
    let resp = http_get_until(host_port, "/", "hello-ferry").await;
    assert!(resp.starts_with("HTTP/1.0 200") || resp.starts_with("HTTP/1.1 200"), "{resp}");

    // ...and from another container on the network through its alias.
    let client =
        ContainerSpec { network: Some(network.clone()), ..cleanup.spec("client", "wget -qO- http://web-alias:8080/") };
    let client_info = docker.run_container(&client).await.unwrap();
    assert_eq!(docker.wait_container(&client_info.id).await.unwrap(), 0);
    let client_logs = collect(docker.logs(&client_info.id, false, None)).await;
    assert!(texts(&client_logs).contains(&"hello-ferry"), "{client_logs:?}");
    docker.remove_container(&client_info.id, false).await.unwrap();

    // Logs: kinds, timestamps, tail.
    let lines = logs_until(&docker, &id, "oops").await;
    let started = lines.iter().find(|l| l.line == "started").expect("stdout line");
    assert_eq!(started.stream, LogStreamKind::Stdout);
    assert!(started.instance.is_none());
    let age = Utc::now() - started.ts;
    assert!(age.num_seconds() >= 0 && age.num_seconds() < 300, "timestamp comes from Docker: {}", started.ts);
    let oops = lines.iter().find(|l| l.line == "oops").expect("stderr line");
    assert_eq!(oops.stream, LogStreamKind::Stderr);
    assert_eq!(collect(docker.logs(&id, false, Some(1))).await.len(), 1);
    assert!(collect(docker.logs(&id, false, Some(0))).await.is_empty());

    // Exec: output of both streams, exit code, working dir, volume content.
    let out = docker.exec(&id, &["sh", "-c", "pwd; cat /data/marker; echo to-stderr >&2; exit 4"]).await.unwrap();
    assert_eq!(out.exit_code, 4);
    assert!(out.output.contains("/www"), "{out:?}");
    assert!(out.output.contains("persisted"), "{out:?}");
    assert!(out.output.contains("to-stderr"), "{out:?}");
    assert_eq!(docker.exec(&id, &["true"]).await.unwrap().exit_code, 0);
    assert!(matches!(docker.exec(&id, &[]).await, Err(Error::Invalid(_))));

    // Stats: one sample honouring the memory limit.
    let stats = docker.stats(&id).await.unwrap();
    assert_eq!(stats.memory_limit_bytes, 64 << 20);
    assert!(stats.memory_bytes > 0);
    assert!(stats.cpu_percent.is_finite() && stats.cpu_percent >= 0.0);

    // The volume is in use: removing it conflicts.
    assert!(matches!(docker.remove_volume(&volume).await, Err(Error::Conflict(_))));

    // Follow: the streams end once the container stops.
    let follow_all = tokio::spawn(collect(docker.logs(&id, true, None)));
    let follow_new = tokio::spawn(collect(docker.logs(&id, true, Some(0))));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!follow_all.is_finished(), "a followed stream stays open while the container runs");
    docker.stop_container(&id, 2).await.unwrap();
    let followed = tokio::time::timeout(Duration::from_secs(20), follow_all).await.expect("follow ends").unwrap();
    assert!(texts(&followed).contains(&"started"), "{followed:?}");
    let followed_new = tokio::time::timeout(Duration::from_secs(20), follow_new).await.expect("follow ends").unwrap();
    assert!(!texts(&followed_new).contains(&"started"), "tail 0 skips history: {followed_new:?}");

    // Stopped: idempotent stop, state, no port, no start time.
    docker.stop_container(&id, 2).await.unwrap();
    let stopped = docker.inspect_container(&id).await.unwrap().unwrap();
    assert_eq!(stopped.state, ContainerState::Exited);
    assert!(stopped.exit_code.is_some());
    assert_eq!(stopped.host_port, None);
    assert_eq!(stopped.started_at, None);
    let listed =
        docker.list_containers(&[("ferry.instance", &cleanup.prefix), ("test", "lifecycle")], false).await.unwrap();
    assert!(listed.is_empty(), "stopped containers are only listed with include_stopped");
    let listed =
        docker.list_containers(&[("ferry.instance", &cleanup.prefix), ("test", "lifecycle")], true).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, ContainerState::Exited);
    assert_eq!(listed[0].exit_code, stopped.exit_code);
    assert!(matches!(docker.exec(&id, &["true"]).await, Err(Error::Conflict(_))));
    let idle = docker.stats(&id).await.unwrap();
    assert_eq!(idle.cpu_percent, 0.0);

    // Restart it: running again with a (possibly new) published port.
    docker.start_container(&id).await.unwrap();
    docker.start_container(&id).await.unwrap();
    let restarted = docker.inspect_container(&id).await.unwrap().unwrap();
    assert_eq!(restarted.state, ContainerState::Running);
    let port = restarted.host_port.expect("host port after restart");
    http_get_until(port, "/", "hello-ferry").await;

    // Removal: a running container needs force; everything is idempotent.
    assert!(matches!(docker.remove_container(&id, false).await, Err(Error::Conflict(_))));
    docker.remove_container(&id, true).await.unwrap();
    docker.remove_container(&id, true).await.unwrap();
    docker.remove_container(&id, false).await.unwrap();
    docker.stop_container(&id, 1).await.unwrap();
    assert!(docker.inspect_container(&id).await.unwrap().is_none());
    assert!(matches!(docker.start_container(&id).await, Err(Error::NotFound(_))));
    assert!(matches!(docker.wait_container(&id).await, Err(Error::NotFound(_))));
    assert!(matches!(docker.stats(&id).await, Err(Error::NotFound(_))));
    assert!(matches!(docker.exec(&id, &["true"]).await, Err(Error::NotFound(_))));
    assert!(collect(docker.logs(&id, false, None)).await.is_empty());
    assert!(collect(docker.logs(&id, true, None)).await.is_empty());

    docker.remove_volume(&volume).await.unwrap();
    docker.remove_volume(&volume).await.unwrap();
    docker.bollard().remove_network(&network).await.unwrap();
    // A network created with the same name again works (and cleanup removes it).
    docker.ensure_network(&network, &labels).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_codes_and_failures() {
    require_e2e!();
    let cleanup = Cleanup::new();
    let docker = connect().await;
    docker.ensure_image(BUSYBOX, &LogSink::noop()).await.unwrap();

    // Exit code through wait, inspect and list.
    let spec =
        ContainerSpec { labels: cleanup.labels(&[("test", "exit")]), ..cleanup.spec("exit3", "echo bye; exit 3") };
    let info = docker.run_container(&spec).await.unwrap();
    assert_eq!(docker.wait_container(&info.id).await.unwrap(), 3);
    assert_eq!(docker.wait_container(&spec.name).await.unwrap(), 3, "waiting on an exited container returns at once");
    let exited = docker.inspect_container(&info.id).await.unwrap().unwrap();
    assert_eq!(exited.state, ContainerState::Exited);
    assert_eq!(exited.exit_code, Some(3));
    let all = docker.list_containers(&[("ferry.instance", &cleanup.prefix), ("test", "exit")], true).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].exit_code, Some(3));
    assert_eq!(texts(&collect(docker.logs(&info.id, true, None)).await), ["bye"], "follow on a stopped container");

    // Name clash.
    match docker.run_container(&spec).await {
        Err(Error::Conflict(msg)) => assert!(msg.contains(&spec.name), "{msg}"),
        other => panic!("expected a conflict, got {other:?}"),
    }

    // Missing image / network → NotFound, nothing left behind.
    let missing_image =
        ContainerSpec { image: format!("{}/missing:none", cleanup.prefix), ..cleanup.spec("noimage", "true") };
    assert!(matches!(docker.run_container(&missing_image).await, Err(Error::NotFound(m)) if m.starts_with("image")));
    assert!(docker.inspect_container(&missing_image.name).await.unwrap().is_none());
    let missing_network = ContainerSpec { network: Some(cleanup.name("nonet")), ..cleanup.spec("nonet", "true") };
    assert!(
        matches!(docker.run_container(&missing_network).await, Err(Error::NotFound(m)) if m.starts_with("network"))
    );
    assert!(docker.inspect_container(&missing_network.name).await.unwrap().is_none());

    // A start failure removes the created container.
    let bad_cmd = ContainerSpec { cmd: Some(vec!["/definitely-not-a-binary".into()]), ..cleanup.spec("badcmd", "") };
    match docker.run_container(&bad_cmd).await {
        Err(Error::Docker(msg)) => assert!(msg.starts_with(&format!("starting container {}", bad_cmd.name)), "{msg}"),
        other => panic!("expected a start failure, got {other:?}"),
    }
    assert!(docker.inspect_container(&bad_cmd.name).await.unwrap().is_none(), "failed container must be removed");

    // Invalid input is rejected before reaching Docker.
    let bad_name = ContainerSpec { name: "bad/name".into(), ..cleanup.spec("x", "true") };
    assert!(matches!(docker.run_container(&bad_name).await, Err(Error::Invalid(_))));

    // A short client timeout does not break calls that block until the container exits.
    let short = Docker::from_bollard(docker.bollard().clone().with_timeout(Duration::from_secs(2)));
    let slow = short.run_container(&cleanup.spec("slow", "echo begin; sleep 4; echo done; exit 5")).await.unwrap();
    let follow = tokio::spawn(collect(short.logs(&slow.id, true, None)));
    assert_eq!(short.wait_container(&slow.id).await.unwrap(), 5);
    let followed = tokio::time::timeout(Duration::from_secs(20), follow).await.expect("follow ends").unwrap();
    assert_eq!(texts(&followed), ["begin", "done"]);
    // Stop with a grace period longer than the client timeout.
    let stubborn = short.run_container(&cleanup.spec("stubborn", "trap '' TERM; sleep 60")).await.unwrap();
    short.stop_container(&stubborn.id, 3).await.unwrap();
    assert_eq!(short.inspect_container(&stubborn.id).await.unwrap().unwrap().state, ContainerState::Exited);

    // A long line is reassembled (the json-file driver splits at 16 KiB).
    let long = docker
        .run_container(&cleanup.spec("longline", "head -c 40000 /dev/zero | tr '\\0' 'x'; echo; echo end"))
        .await
        .unwrap();
    assert_eq!(docker.wait_container(&long.id).await.unwrap(), 0);
    let lines = collect(docker.logs(&long.id, false, None)).await;
    assert_eq!(lines.len(), 2, "{:?}", lines.iter().map(|l| l.line.len()).collect::<Vec<_>>());
    assert_eq!(lines[0].line.len(), 40000);
    assert!(lines[0].line.bytes().all(|b| b == b'x'));
    assert_eq!(lines[1].line, "end");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn images() {
    require_e2e!();
    let mut cleanup = Cleanup::new();
    let docker = connect().await;

    assert!(docker.image_exists(BUSYBOX).await.unwrap());
    assert!(!docker.image_exists(&format!("{}/nope:1", cleanup.prefix)).await.unwrap());
    assert_eq!(docker.image_exposed_ports("nginx:alpine").await.unwrap(), vec![80]);
    assert!(docker.image_exposed_ports(BUSYBOX).await.unwrap().is_empty());
    assert!(matches!(docker.image_exposed_ports(&format!("{}/nope:1", cleanup.prefix)).await, Err(Error::NotFound(_))));
    assert!(matches!(docker.image_exists("../x").await, Err(Error::Invalid(_))));

    // Our own tag of busybox: removable, idempotently.
    let repo = format!("{}/img", cleanup.prefix);
    let tagged = format!("{repo}:v1");
    cleanup.images.push(tagged.clone());
    let options = bollard::query_parameters::TagImageOptionsBuilder::new().repo(&repo).tag("v1").build();
    docker.bollard().tag_image(BUSYBOX, Some(options)).await.unwrap();
    assert!(docker.image_exists(&tagged).await.unwrap());
    docker.remove_image(&tagged).await.unwrap();
    assert!(!docker.image_exists(&tagged).await.unwrap());
    docker.remove_image(&tagged).await.unwrap();
    assert!(docker.image_exists(BUSYBOX).await.unwrap(), "untagging must not touch the base image");

    // Pull by digest: no tag changes, needs the registry.
    let inspect = docker.bollard().inspect_image(BUSYBOX).await.unwrap();
    // Other tags of the same image (made by concurrent tests) add their own
    // repo digests, e.g. `ferryfix-docker-…/x@sha256:…`: pick busybox's.
    let digest_ref = inspect
        .repo_digests
        .unwrap_or_default()
        .into_iter()
        .find(|d| d.starts_with("busybox@") || d.starts_with("docker.io/library/busybox@"))
        .expect("busybox repo digest");
    let (sink, mut rx) = LogSink::channel();
    docker.pull_image(&digest_ref, &sink).await.unwrap();
    drop(sink);
    let mut lines = Vec::new();
    while let Some(line) = rx.recv().await {
        lines.push(line);
    }
    let last = lines.last().expect("pull output");
    assert_eq!(last.line, format!("==> Pulled {digest_ref}"));
    assert_eq!(last.stream, LogStreamKind::System);
    assert!(lines.len() < 50, "one line per status change, not per tick: {lines:?}");

    // Unknown repository.
    let missing = format!("{}/does-not-exist:latest", cleanup.prefix);
    match docker.pull_image(&missing, &LogSink::noop()).await {
        Err(Error::NotFound(_)) | Err(Error::Docker(_)) => {}
        other => panic!("expected a pull failure, got {other:?}"),
    }
    assert!(matches!(docker.pull_image("bad image", &LogSink::noop()).await, Err(Error::Invalid(_))));
}

/// `tag_image`, `volume_exists`, and exec errors that never reveal the
/// command's arguments or env values (regression: a readiness probe's
/// `redis-cli -a <password>` ended up in error messages).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tags_volumes_and_exec_secrets() {
    require_e2e!();
    let mut cleanup = Cleanup::with_prefix("ferryfix-docker");
    let docker = connect().await;
    docker.ensure_image(BUSYBOX, &LogSink::noop()).await.unwrap();

    // tag_image: a second name for the same local image.
    let tagged = format!("{}/x:1", cleanup.prefix);
    cleanup.images.push(tagged.clone());
    assert!(!docker.image_exists(&tagged).await.unwrap());
    docker.tag_image(BUSYBOX, &tagged).await.unwrap();
    assert!(docker.image_exists(&tagged).await.unwrap());
    let source_id = docker.bollard().inspect_image(BUSYBOX).await.unwrap().id;
    assert_eq!(docker.bollard().inspect_image(&tagged).await.unwrap().id, source_id);
    docker.tag_image(BUSYBOX, &tagged).await.unwrap();
    // No tag means `latest`.
    let untagged = format!("{}/y", cleanup.prefix);
    cleanup.images.push(format!("{untagged}:latest"));
    docker.tag_image(BUSYBOX, &untagged).await.unwrap();
    assert!(docker.image_exists(&format!("{untagged}:latest")).await.unwrap());
    docker.remove_image(&untagged).await.unwrap();
    // Errors: missing source, digest target.
    let missing = format!("{}/missing:1", cleanup.prefix);
    match docker.tag_image(&missing, &tagged).await {
        Err(Error::NotFound(m)) => assert_eq!(m, format!("image '{missing}'")),
        other => panic!("expected NotFound for a missing source, got {other:?}"),
    }
    let digest = source_id.clone().expect("image id");
    assert!(matches!(
        docker.tag_image(BUSYBOX, &format!("{}/x@{digest}", cleanup.prefix)).await,
        Err(Error::Invalid(_))
    ));
    docker.remove_image(&tagged).await.unwrap();
    assert!(!docker.image_exists(&tagged).await.unwrap());
    assert!(docker.image_exists(BUSYBOX).await.unwrap(), "untagging must not touch the source image");

    // volume_exists: false → true → false.
    let volume = cleanup.name("vol");
    cleanup.volumes.push(volume.clone());
    assert!(!docker.volume_exists(&volume).await.unwrap());
    docker.ensure_volume(&volume, &cleanup.labels(&[])).await.unwrap();
    assert!(docker.volume_exists(&volume).await.unwrap());
    docker.remove_volume(&volume).await.unwrap();
    assert!(!docker.volume_exists(&volume).await.unwrap());
    assert!(matches!(docker.volume_exists("../x").await, Err(Error::Invalid(_))));

    // exec_with_env: the value reaches the command without being in argv.
    let secret = "s3cret-value";
    let info = docker.run_container(&cleanup.spec("exec", "sleep 300")).await.unwrap();
    let out = docker
        .exec_with_env(
            &info.id,
            &["sh", "-c", "echo \"auth=$FERRYFIX_SECRET path=$PATH\""],
            &[("FERRYFIX_SECRET", secret)],
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.output.starts_with(&format!("auth={secret} path=/")), "container env is kept: {out:?}");
    let args = docker.exec(&info.id, &["sh", "-c", "echo \"[$FERRYFIX_SECRET]\""]).await.unwrap();
    assert_eq!(args.output.trim(), "[]", "env is per exec");

    // Errors name the program only.
    docker.stop_container(&info.id, 1).await.unwrap();
    for result in [
        docker.exec(&info.id, &["redis-cli", "--no-auth-warning", "-a", secret, "ping"]).await,
        docker.exec_with_env(&info.id, &["redis-cli", "ping"], &[("REDISCLI_AUTH", secret)]).await,
    ] {
        match result {
            Err(Error::Conflict(m)) => {
                assert!(m.starts_with(&format!("running \"redis-cli\" in container {}", info.id)), "{m}");
                assert!(!m.contains(secret), "exec error reveals the secret: {m}");
            }
            other => panic!("expected a conflict on a stopped container, got {other:?}"),
        }
    }
    docker.remove_container(&info.id, true).await.unwrap();
    let err = docker.exec(&info.id, &["redis-cli", "-a", secret, "ping"]).await.unwrap_err();
    assert!(matches!(&err, Error::NotFound(_)), "{err:?}");
    assert!(!err.to_string().contains(secret), "{err}");
}

/// The container's own cgroup v2 limits (`memory.max`, `cpu.max`,
/// `pids.max`), or `None` on a cgroup v1 host.
async fn cgroup_limits(docker: &Docker, id: &str) -> Option<[String; 3]> {
    let out = docker
        .exec(id, &["cat", "/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/cpu.max", "/sys/fs/cgroup/pids.max"])
        .await
        .unwrap();
    if out.exit_code != 0 {
        return None;
    }
    let lines: Vec<String> = out.output.lines().map(|l| l.trim().to_string()).collect();
    Some(lines.try_into().unwrap_or_else(|l| panic!("unexpected cgroup output: {l:?}")))
}

/// `(Memory, MemorySwap, NanoCpus, PidsLimit)` of a container's HostConfig.
async fn host_limits(docker: &Docker, id: &str) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    let hc = docker.bollard().inspect_container(id, None).await.unwrap().host_config.unwrap();
    (hc.memory, hc.memory_swap, hc.nano_cpus, hc.pids_limit)
}

/// `host_info`, and what the daemon accepts at creation: limits, pids limit
/// and log rotation are applied; a CPU quota above the host's CPU count is
/// refused, a memory limit above the host's memory is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limits_on_create() {
    require_e2e!();
    let cleanup = Cleanup::new();
    let docker = connect().await;
    docker.ensure_image(BUSYBOX, &LogSink::noop()).await.unwrap();

    let host = docker.host_info().await.unwrap();
    let cpus = host.cpus.expect("host CPUs");
    let memory = host.memory_bytes.expect("host memory");
    assert!(memory > 64 << 20, "{host:?}");
    assert!(host.docker_root_dir.as_deref().is_some_and(|d| d.starts_with('/')), "{host:?}");
    assert!(host.operating_system.is_some(), "{host:?}");
    let cli = Command::new("docker").args(["info", "--format", "{{.NCPU}} {{.MemTotal}}"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&cli.stdout).trim(), format!("{cpus} {memory}"), "same as `docker info`");

    let spec = ContainerSpec {
        memory_limit_bytes: Some(64 << 20),
        nano_cpus: Some(500_000_000),
        pids_limit: Some(128),
        log_rotation: Some(LogRotation { max_size_mb: 1, max_files: 2 }),
        ..cleanup.spec("limited", "seq 1 100000; echo end; exec sleep 300")
    };
    let info = docker.run_container(&spec).await.unwrap();
    assert_eq!(info.memory_limit_bytes, Some(64 << 20));
    assert_eq!(info.nano_cpus, Some(500_000_000));
    assert!(!info.oom_killed);
    assert_eq!(host_limits(&docker, &info.id).await, (Some(64 << 20), Some(64 << 20), Some(500_000_000), Some(128)));
    let hc = docker.bollard().inspect_container(&info.id, None).await.unwrap().host_config.unwrap();
    let log_config = hc.log_config.expect("log config");
    assert_eq!(log_config.typ.as_deref(), Some("json-file"));
    let options = log_config.config.unwrap_or_default();
    assert_eq!((options["max-size"].as_str(), options["max-file"].as_str()), ("1m", "2"));
    if let Some([mem, cpu, pids]) = cgroup_limits(&docker, &info.id).await {
        assert_eq!((mem.as_str(), cpu.as_str(), pids.as_str()), ("67108864", "50000 100000", "128"));
    }
    // Rotation: ~10 MB of json-file log was written, at most ~2 MiB is kept.
    let lines = logs_until(&docker, &info.id, "end").await;
    assert!(lines.len() < 100_000, "{} lines kept", lines.len());
    assert_ne!(lines.first().map(|l| l.line.as_str()), Some("1"), "the oldest lines were rotated away");
    // The pids limit holds: forking past it fails.
    let fork = docker.exec(&info.id, &["sh", "-c", "for i in $(seq 1 200); do sleep 30 & done; wait"]).await.unwrap();
    assert_ne!(fork.exit_code, 0, "{fork:?}");
    assert!(fork.output.contains("fork") || fork.output.contains("resource"), "{fork:?}");

    // No limits at all: nothing is set, inspect reports None.
    let free = docker.run_container(&cleanup.spec("free", "exec sleep 300")).await.unwrap();
    assert_eq!((free.memory_limit_bytes, free.nano_cpus), (None, None));
    let (mem, swap, nano, pids) = host_limits(&docker, &free.id).await;
    assert_eq!((mem, swap, nano), (Some(0), Some(0), Some(0)));
    assert!(pids.is_none_or(|p| p <= 0), "{pids:?}");

    // More CPUs than the host has: refused (the engine caps the quota).
    let too_many = ContainerSpec {
        nano_cpus: Some(i64::from(cpus + 1) * 1_000_000_000),
        ..cleanup.spec("cpus", "exec sleep 300")
    };
    match docker.run_container(&too_many).await {
        Err(Error::Docker(m)) => assert!(m.to_lowercase().contains("range of cpus"), "{m}"),
        other => panic!("expected the daemon to refuse the CPU quota, got {other:?}"),
    }
    assert!(docker.inspect_container(&too_many.name).await.unwrap().is_none());
    // More memory than the host has: accepted.
    let big = ContainerSpec {
        memory_limit_bytes: Some(i64::try_from(memory).unwrap() + (1 << 30)),
        ..cleanup.spec("bigmem", "exec sleep 300")
    };
    let big = docker.run_container(&big).await.unwrap();
    assert_eq!(big.memory_limit_bytes, Some(i64::try_from(memory).unwrap() + (1 << 30)));
}

/// `update_limits` changes a live container's limits without a restart and
/// really removes them with `None` (see `convert::update_body` for the
/// encoding Docker needs).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_limits_in_place() {
    require_e2e!();
    let cleanup = Cleanup::new();
    let docker = connect().await;
    docker.ensure_image(BUSYBOX, &LogSink::noop()).await.unwrap();
    let cpus = docker.host_info().await.unwrap().cpus.expect("host CPUs");
    let all_cpus = i64::from(cpus) * 1_000_000_000;

    // Created without limits, then limited.
    let info = docker.run_container(&cleanup.spec("db", "exec sleep 300")).await.unwrap();
    let id = info.id.clone();
    let limited =
        LimitsUpdate { memory_limit_bytes: Some(96 << 20), nano_cpus: Some(250_000_000), pids_limit: Some(64) };
    docker.update_limits(&id, limited).await.unwrap();
    let after = docker.inspect_container(&id).await.unwrap().unwrap();
    assert_eq!(after.started_at, info.started_at, "no restart");
    assert_eq!((after.memory_limit_bytes, after.nano_cpus), (Some(96 << 20), Some(250_000_000)));
    assert_eq!(host_limits(&docker, &id).await, (Some(96 << 20), Some(96 << 20), Some(250_000_000), Some(64)));
    if let Some([mem, cpu, pids]) = cgroup_limits(&docker, &id).await {
        assert_eq!((mem.as_str(), cpu.as_str(), pids.as_str()), ("100663296", "25000 100000", "64"));
    }

    // Changed.
    let changed = LimitsUpdate { memory_limit_bytes: Some(48 << 20), nano_cpus: Some(1_000_000_000), pids_limit: None };
    docker.update_limits(&id, changed).await.unwrap();
    assert_eq!(host_limits(&docker, &id).await, (Some(48 << 20), Some(48 << 20), Some(1_000_000_000), Some(0)));

    // Removed: no memory limit (reported as None), a quota of every host CPU,
    // no pids limit — and the kernel agrees.
    docker.update_limits(&id, LimitsUpdate::default()).await.unwrap();
    let removed = docker.inspect_container(&id).await.unwrap().unwrap();
    assert_eq!(removed.state, ContainerState::Running);
    assert_eq!(removed.memory_limit_bytes, None);
    assert_eq!(removed.nano_cpus, Some(all_cpus));
    assert_eq!(host_limits(&docker, &id).await.3, Some(0));
    if let Some([mem, cpu, pids]) = cgroup_limits(&docker, &id).await {
        assert!(mem == "max" || mem.parse::<u64>().is_ok_and(|m| m >= 1 << 60), "memory.max {mem}");
        assert_eq!(cpu, format!("{} 100000", u64::from(cpus) * 100_000));
        assert_eq!(pids, "max");
    }

    // Limited again after the removal; then removed on a stopped container,
    // which must still start (the sentinel survives Docker Desktop's proxy).
    docker.update_limits(&id, limited).await.unwrap();
    assert_eq!(host_limits(&docker, &id).await, (Some(96 << 20), Some(96 << 20), Some(250_000_000), Some(64)));
    docker.stop_container(&id, 1).await.unwrap();
    docker.update_limits(&id, LimitsUpdate::default()).await.unwrap();
    docker.start_container(&id).await.unwrap();
    let restarted = docker.inspect_container(&id).await.unwrap().unwrap();
    assert_eq!(restarted.state, ContainerState::Running);
    assert_eq!((restarted.memory_limit_bytes, restarted.nano_cpus), (None, Some(all_cpus)));
    let restart = Command::new("docker").args(["restart", "-t", "1", &id]).output().unwrap();
    assert!(restart.status.success(), "docker restart: {}", String::from_utf8_lossy(&restart.stderr));
    assert_eq!(docker.inspect_container(&id).await.unwrap().unwrap().state, ContainerState::Running);

    // Errors.
    let too_many = LimitsUpdate { nano_cpus: Some(all_cpus + 1_000_000_000), ..limited };
    match docker.update_limits(&id, too_many).await {
        Err(Error::Docker(m)) => assert!(m.to_lowercase().contains("range of cpus"), "{m}"),
        other => panic!("expected the daemon to refuse the CPU quota, got {other:?}"),
    }
    docker.remove_container(&id, true).await.unwrap();
    assert!(matches!(docker.update_limits(&id, limited).await, Err(Error::NotFound(_))));
}

/// A container killed by the kernel for going over its memory limit is
/// reported `oom_killed`; one killed by a plain SIGKILL (same exit code
/// 137) is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oom_kills_are_reported() {
    require_e2e!();
    let cleanup = Cleanup::new();
    let docker = connect().await;
    docker.ensure_image(BUSYBOX, &LogSink::noop()).await.unwrap();

    // `tail` buffers a newline-free /dev/zero until the limit is hit.
    let hog = ContainerSpec { memory_limit_bytes: Some(32 << 20), ..cleanup.spec("hog", "exec tail /dev/zero") };
    let info = docker.run_container(&hog).await.unwrap();
    let code = tokio::time::timeout(Duration::from_secs(60), docker.wait_container(&info.id))
        .await
        .expect("the container is OOM-killed quickly")
        .unwrap();
    assert_eq!(code, 137);
    let dead = docker.inspect_container(&info.id).await.unwrap().unwrap();
    assert_eq!(dead.state, ContainerState::Exited);
    assert_eq!(dead.exit_code, Some(137));
    assert!(dead.oom_killed, "{dead:?}");
    assert_eq!(dead.memory_limit_bytes, Some(32 << 20));
    let listed = docker.list_containers(&[("ferry.instance", &cleanup.prefix)], true).await.unwrap();
    assert!(listed.iter().all(|c| !c.oom_killed), "the list API never reports OOM kills");

    // SIGKILL without OOM: same exit code, not an OOM kill.
    let killed = docker.run_container(&cleanup.spec("killed", "exec sleep 300")).await.unwrap();
    docker.stop_container(&killed.id, 0).await.unwrap();
    let killed = docker.inspect_container(&killed.id).await.unwrap().unwrap();
    assert_eq!(killed.exit_code, Some(137));
    assert!(!killed.oom_killed);
    assert_eq!((killed.memory_limit_bytes, killed.nano_cpus), (None, None));
}
