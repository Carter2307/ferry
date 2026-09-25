//! Docker-gated end-to-end tests. They only run with `FERRY_E2E=1` (otherwise
//! each test prints "skipped" and returns). Every Docker object they create is
//! prefixed `ferrytest-docker-<random>`, labelled `ferry.instance=<prefix>`,
//! and removed by a drop guard even when an assertion fails.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use ferry_core::{Error, LogLine, LogSink, LogStreamKind, ids};
use ferry_docker::{ContainerSpec, ContainerState, Docker, PortPublish, RestartPolicy, VolumeMount};
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
        Cleanup {
            prefix: format!("ferrytest-docker-{}", ids::random_secret(8)),
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
    let digest_ref = inspect.repo_digests.unwrap_or_default().into_iter().next().expect("busybox repo digest");
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
