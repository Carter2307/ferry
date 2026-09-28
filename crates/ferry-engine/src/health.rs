//! Readiness probes: health checks of new instances and the TCP check that
//! decides which instances receive traffic.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ferry_core::{LogSink, Service};
use ferry_docker::{ContainerInfo, ContainerState};
use futures::StreamExt;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::state::Inner;

/// How often a new instance is probed.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often a progress line is logged while an instance is not healthy.
pub(crate) const PROGRESS_EVERY: Duration = Duration::from_secs(10);
/// Looking for the exit code of a crashed instance Docker restarts.
const EXIT_CODE_POLLS: usize = 30;
const EXIT_CODE_POLL: Duration = Duration::from_millis(100);
/// Bound on reading a container's past events from Docker.
const EVENTS_TIMEOUT: Duration = Duration::from_secs(5);
/// Workers must still be running this long after start.
pub(crate) const WORKER_MIN_UPTIME: Duration = Duration::from_secs(5);
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// After connecting, the connection must stay open this long (or deliver
/// data). Docker's port forwarders accept connections on published ports
/// even when nothing listens inside the container and close them right
/// away, so a successful `connect` alone proves nothing.
pub(crate) const TCP_SETTLE: Duration = Duration::from_millis(300);

/// How a new instance is judged healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// `GET http://127.0.0.1:<host_port><path>` with `Host: <host>` → < 400.
    Http { path: String, host: String },
    /// A TCP connection to `127.0.0.1:<host_port>` is accepted and held.
    Tcp,
    /// Still running [`WORKER_MIN_UPTIME`] after start.
    Uptime,
}

impl Probe {
    pub(crate) fn for_service(svc: &Service, default_host: String) -> Probe {
        if !svc.listens() {
            return Probe::Uptime;
        }
        match svc.health_check_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(path) => {
                let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
                Probe::Http { path, host: default_host }
            }
            None => Probe::Tcp,
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Probe::Http { path, .. } => format!("GET {path} must answer with a status below 400"),
            Probe::Tcp => "the port must accept TCP connections".to_string(),
            Probe::Uptime => format!("the process must keep running for {}s", WORKER_MIN_UPTIME.as_secs()),
        }
    }
}

/// True when `127.0.0.1:<port>` accepts a TCP connection and doesn't close
/// it immediately (see [`TCP_SETTLE`]).
pub(crate) async fn tcp_accepting(port: u16, settle: Duration) -> bool {
    let connect = TcpStream::connect(("127.0.0.1", port));
    let Ok(Ok(mut stream)) = tokio::time::timeout(TCP_CONNECT_TIMEOUT, connect).await else {
        return false;
    };
    let mut buf = [0u8; 1];
    match tokio::time::timeout(settle, stream.read(&mut buf)).await {
        // Still open and silent: a real listener waiting for a request.
        Err(_) => true,
        // A server that speaks first.
        Ok(Ok(n)) => n > 0,
        // Closed / reset right away: nothing listens behind the forwarder.
        Ok(Err(_)) => false,
    }
}

/// Why an HTTP probe failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeError {
    /// The app answered with an error status.
    Status(String),
    /// No answer at all (refused, closed, timed out).
    Connection(String),
}

/// One HTTP health probe.
pub(crate) async fn http_probe(client: &reqwest::Client, port: u16, path: &str, host: &str) -> Result<(), ProbeError> {
    let url = format!("http://127.0.0.1:{port}{path}");
    match client.get(&url).header(reqwest::header::HOST, host).send().await {
        Ok(resp) if resp.status().as_u16() < 400 => Ok(()),
        Ok(resp) => Err(ProbeError::Status(format!("GET {path} returned HTTP {}", resp.status().as_u16()))),
        Err(e) if e.is_timeout() => Err(ProbeError::Connection(format!("GET {path} timed out"))),
        Err(e) if e.is_connect() => Err(ProbeError::Connection(format!("GET {path}: connection refused"))),
        Err(_) => Err(ProbeError::Connection(format!("GET {path}: the connection was closed without a response"))),
    }
}

/// Why an instance did not become healthy.
#[derive(Debug, Clone)]
pub(crate) struct InstanceFailure {
    pub container: ContainerInfo,
    pub message: String,
    /// The process exited (its last output is worth showing).
    pub crashed: bool,
}

/// How the new instances of a deploy are checked.
pub(crate) struct HealthCheck {
    pub probe: Probe,
    pub deadline: Instant,
    pub timeout_secs: u64,
    /// The port the app must listen on inside the container.
    pub container_port: Option<u16>,
    /// Appended to connection failures on timeout (e.g. "does the app listen
    /// on $PORT (8000)?").
    pub port_hint: Option<String>,
    /// Background workers (a clean exit is a mistake worth explaining).
    pub worker: bool,
    /// Where progress lines go while waiting.
    pub progress: LogSink,
}

/// What an instance's exit code usually means. (137 = SIGKILL: an OOM kill
/// is recognized by Docker's `OOMKilled` flag instead, see [`crash_message`].)
fn exit_code_hint(code: i64) -> &'static str {
    match code {
        126 => ": the command is not executable",
        127 => ": command not found, check the start command",
        137 => ": killed by SIGKILL",
        139 => ": segmentation fault",
        _ => "",
    }
}

/// How a crashed instance's process last ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LastExit {
    pub code: Option<i64>,
    /// The kernel killed it for exceeding its memory limit.
    pub oom_killed: bool,
    /// The container's memory limit (bytes), if any.
    pub memory_limit_bytes: Option<i64>,
}

/// The message for a crashed instance: out of memory (naming the limit), or
/// its exit code.
pub(crate) fn crash_message(instance: &str, exit: LastExit, worker: bool) -> String {
    if exit.oom_killed {
        crate::limits::oom_message(&format!("instance {instance}"), exit.memory_limit_bytes, "the service's")
    } else {
        exit_message(instance, exit.code, worker)
    }
}

/// The message for an instance whose process ended with `code`.
pub(crate) fn exit_message(instance: &str, code: Option<i64>, worker: bool) -> String {
    match code {
        Some(0) if worker => format!("instance {instance} exited with code 0 (a background worker must keep running)"),
        Some(0) => format!("instance {instance} exited with code 0 (the process must keep running)"),
        Some(c) => format!("instance {instance} crashed (exit code {c}{})", exit_code_hint(c)),
        None => format!("instance {instance} crashed"),
    }
}

/// How a container that Docker restarts (the restart policy brings crashed
/// instances back) last exited. A running container reports exit code 0 and
/// no OOM kill, so this waits briefly for it to be seen restarting or
/// stopped. When it isn't (Docker restarts a crashed container within
/// ~100 ms, and an app that runs a while before crashing again stays up
/// longer than this waits), the container's Docker events since `started`
/// (when it was started, as a time since the epoch) tell instead.
/// `memory_limit_bytes`: its memory limit.
async fn last_exit(inner: &Inner, id: &str, started: Duration, memory_limit_bytes: Option<i64>) -> LastExit {
    for _ in 0..EXIT_CODE_POLLS {
        let Ok(resp) = inner.docker.bollard().inspect_container(id, None).await else { break };
        let Some(state) = resp.state else { break };
        let restarting = state.restarting == Some(true);
        let stopped = matches!(state.status.map(|s| s.to_string()).as_deref(), Some("exited" | "dead" | "restarting"));
        if restarting || stopped {
            return LastExit {
                code: state.exit_code,
                oom_killed: state.oom_killed.unwrap_or(false),
                memory_limit_bytes: resp.host_config.and_then(|h| h.memory).filter(|m| *m > 0),
            };
        }
        tokio::time::sleep(EXIT_CODE_POLL).await;
    }
    exit_from_events(inner, id, started, memory_limit_bytes).await
}

/// How the container last exited according to its Docker events (`die`,
/// with its exit code, and `oom`) since `since` (a time since the epoch).
async fn exit_from_events(inner: &Inner, id: &str, since: Duration, memory_limit_bytes: Option<i64>) -> LastExit {
    let filters: HashMap<String, Vec<String>> = HashMap::from([
        ("type".to_string(), vec!["container".to_string()]),
        ("container".to_string(), vec![id.to_string()]),
        ("event".to_string(), vec!["die".to_string(), "oom".to_string()]),
    ]);
    let since_ns = i64::try_from(since.as_nanos()).unwrap_or(i64::MAX);
    // With `until`, Docker sends the past events and closes the stream.
    let options = bollard::query_parameters::EventsOptionsBuilder::default()
        .filters(&filters)
        .since(&crate::reconcile::events_since(since_ns))
        .until(&crate::reconcile::events_since(crate::reconcile::now_ns()))
        .build();
    let mut events = inner.docker.bollard().events(Some(options));
    let mut exit = LastExit { memory_limit_bytes, ..LastExit::default() };
    let read = async {
        while let Some(Ok(event)) = events.next().await {
            let attributes = event.actor.and_then(|a| a.attributes).unwrap_or_default();
            exit_event(&mut exit, event.action.as_deref(), &attributes);
        }
    };
    if tokio::time::timeout(EVENTS_TIMEOUT, read).await.is_err() {
        tracing::debug!(container = id, "reading the container's events timed out");
    }
    exit
}

/// Fold one Docker event (`action`, `attributes`) into how the container
/// last exited: any `oom` is an OOM kill; the last `die` has the exit code.
fn exit_event(exit: &mut LastExit, action: Option<&str>, attributes: &HashMap<String, String>) {
    match action {
        Some("oom") => exit.oom_killed = true,
        Some("die") => exit.code = attributes.get("exitCode").and_then(|c| c.parse().ok()).or(exit.code),
        _ => {}
    }
}

/// Probe one new instance every second until it is healthy, it crashes or
/// the check's deadline passes. `started` is when it was started. Returns
/// the host port that was verified (`None` for workers). A progress line
/// is logged every [`PROGRESS_EVERY`] while waiting.
pub(crate) async fn wait_healthy(
    inner: &Inner,
    check: &HealthCheck,
    container: &ContainerInfo,
    started: Instant,
) -> Result<Option<u16>, InstanceFailure> {
    let instance = crate::util::instance_id(&container.name);
    let fail = |message: String, crashed: bool| InstanceFailure { container: container.clone(), message, crashed };
    let port_text = |host_port: u16| match check.container_port {
        Some(p) => format!("container port {p} (published as 127.0.0.1:{host_port})"),
        None => format!("127.0.0.1:{host_port}"),
    };
    let mut next_progress = Instant::now() + PROGRESS_EVERY;
    // When it was started (since the epoch, with a second's slack), for
    // reading its Docker events.
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let started_at = since_epoch.saturating_sub(started.elapsed() + Duration::from_secs(1));
    loop {
        // (reason, whether it is a connection problem the port hint explains)
        let (last, connection): (String, bool) = match inner.docker.inspect_container(&container.id).await {
            Ok(None) => return Err(fail(format!("instance {instance} was removed"), false)),
            Ok(Some(info)) => match info.state {
                ContainerState::Exited | ContainerState::Dead => {
                    let exit = LastExit {
                        code: info.exit_code,
                        oom_killed: info.oom_killed,
                        memory_limit_bytes: info.memory_limit_bytes,
                    };
                    return Err(fail(crash_message(&instance, exit, check.worker), true));
                }
                ContainerState::Restarting => {
                    let exit = last_exit(inner, &container.id, started_at, info.memory_limit_bytes).await;
                    return Err(fail(crash_message(&instance, exit, check.worker), true));
                }
                ContainerState::Running if info.restart_count.unwrap_or(0) > 0 => {
                    let exit = last_exit(inner, &container.id, started_at, info.memory_limit_bytes).await;
                    return Err(fail(crash_message(&instance, exit, check.worker), true));
                }
                ContainerState::Running => match &check.probe {
                    Probe::Uptime => {
                        if started.elapsed() >= WORKER_MIN_UPTIME {
                            return Ok(None);
                        }
                        (format!("instance {instance} has not been up for {}s yet", WORKER_MIN_UPTIME.as_secs()), false)
                    }
                    Probe::Tcp => match info.host_port.or(container.host_port) {
                        Some(port) if tcp_accepting(port, TCP_SETTLE).await => return Ok(Some(port)),
                        Some(port) => (format!("nothing accepts connections on {} yet", port_text(port)), true),
                        None => return Err(fail(format!("instance {instance} has no published port"), false)),
                    },
                    Probe::Http { path, host } => match (info.host_port.or(container.host_port), &inner.http) {
                        (Some(port), Ok(client)) => match http_probe(client, port, path, host).await {
                            Ok(()) => return Ok(Some(port)),
                            Err(ProbeError::Status(reason)) => (reason, false),
                            Err(ProbeError::Connection(reason)) => (format!("{reason} on {}", port_text(port)), true),
                        },
                        (None, _) => return Err(fail(format!("instance {instance} has no published port"), false)),
                        (_, Err(e)) => return Err(fail(e.clone(), false)),
                    },
                },
                other => (format!("instance {instance} is {}", other.as_str()), false),
            },
            Err(e) => (format!("inspecting instance {instance}: {e}"), false),
        };
        let now = Instant::now();
        if now >= check.deadline {
            let hint = match (&check.port_hint, connection) {
                (Some(h), true) => format!(": {h}"),
                _ => String::new(),
            };
            return Err(fail(format!("health check timed out after {}s: {last}{hint}", check.timeout_secs), false));
        }
        if now >= next_progress {
            next_progress = now + PROGRESS_EVERY;
            let left = check.deadline.saturating_duration_since(now).as_secs();
            check.progress.system(format!("==> Instance {instance} is not healthy yet: {last} ({left}s left)"));
        }
        tokio::time::sleep(POLL_INTERVAL.min(check.deadline - now)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::ServiceType;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    #[test]
    fn probe_selection() {
        let mut web = Service::new("web", ServiceType::WebService);
        assert_eq!(Probe::for_service(&web, "web.localhost".into()), Probe::Tcp);
        web.health_check_path = Some("healthz".into());
        assert_eq!(
            Probe::for_service(&web, "web.localhost".into()),
            Probe::Http { path: "/healthz".into(), host: "web.localhost".into() }
        );
        let worker = Service::new("w", ServiceType::BackgroundWorker);
        assert_eq!(Probe::for_service(&worker, String::new()), Probe::Uptime);
        let mut pserv = Service::new("p", ServiceType::PrivateService);
        pserv.health_check_path = Some("/up".into());
        assert!(matches!(Probe::for_service(&pserv, String::new()), Probe::Http { .. }));
        assert!(Probe::Tcp.describe().contains("TCP"));
    }

    #[test]
    fn exit_messages_explain_the_exit_code() {
        assert_eq!(
            exit_message("abc123", Some(0), true),
            "instance abc123 exited with code 0 (a background worker must keep running)"
        );
        assert!(exit_message("abc123", Some(0), false).contains("exited with code 0"));
        assert_eq!(
            exit_message("abc123", Some(127), false),
            "instance abc123 crashed (exit code 127: command not found, check the start command)"
        );
        assert_eq!(exit_message("abc123", Some(3), false), "instance abc123 crashed (exit code 3)");
        assert_eq!(exit_message("abc123", None, false), "instance abc123 crashed");
        // 137 alone is a SIGKILL, not proof of an OOM kill.
        assert_eq!(
            exit_message("abc123", Some(137), false),
            "instance abc123 crashed (exit code 137: killed by SIGKILL)"
        );
    }

    #[test]
    fn oom_kills_name_the_memory_limit() {
        let oom = LastExit { code: Some(137), oom_killed: true, memory_limit_bytes: Some(32 << 20) };
        assert_eq!(
            crash_message("abc123", oom, false),
            "instance abc123 ran out of memory (limit 32 MiB) — raise the service's memory limit"
        );
        let killed = LastExit { oom_killed: false, ..oom };
        assert_eq!(crash_message("abc123", killed, true), exit_message("abc123", Some(137), true));
        assert_eq!(crash_message("abc123", LastExit::default(), false), "instance abc123 crashed");
    }

    #[test]
    fn crashes_are_read_from_docker_events() {
        let attrs = |code: &str| HashMap::from([("exitCode".to_string(), code.to_string())]);
        let mut exit = LastExit { memory_limit_bytes: Some(512 << 20), ..LastExit::default() };
        exit_event(&mut exit, Some("start"), &HashMap::new());
        assert_eq!(exit, LastExit { memory_limit_bytes: Some(512 << 20), ..LastExit::default() });
        exit_event(&mut exit, Some("oom"), &HashMap::new());
        exit_event(&mut exit, Some("die"), &attrs("137"));
        assert_eq!(exit, LastExit { code: Some(137), oom_killed: true, memory_limit_bytes: Some(512 << 20) });
        assert_eq!(
            crash_message("ab12cd", exit, false),
            "instance ab12cd ran out of memory (limit 512 MiB) — raise the service's memory limit"
        );
        // The last exit code wins; an unreadable one keeps the previous.
        let mut exit = LastExit::default();
        exit_event(&mut exit, Some("die"), &attrs("1"));
        exit_event(&mut exit, Some("die"), &attrs("?"));
        assert_eq!(exit, LastExit { code: Some(1), ..LastExit::default() });
        exit_event(&mut exit, Some("die"), &attrs("2"));
        assert_eq!(crash_message("ab12cd", exit, false), "instance ab12cd crashed (exit code 2)");
    }

    #[tokio::test]
    async fn tcp_probe_distinguishes_listeners_from_closing_forwarders() {
        // A real listener that waits for the client.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = tokio::spawn(async move {
            let mut conns = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                conns.push(s);
            }
        });
        assert!(tcp_accepting(port, Duration::from_millis(200)).await);
        hold.abort();

        // A "forwarder" that accepts and closes immediately.
        let closer = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closer.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((s, _)) = closer.accept().await {
                drop(s);
            }
        });
        assert!(!tcp_accepting(port, Duration::from_millis(500)).await);
        task.abort();

        // A server that speaks first.
        let talker = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = talker.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut s, _)) = talker.accept().await {
                let _ = s.write_all(b"220 hello\r\n").await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        assert!(tcp_accepting(port, Duration::from_millis(500)).await);
        task.abort();

        // Nothing at all.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        assert!(!tcp_accepting(port, Duration::from_millis(100)).await);
    }

    #[tokio::test]
    async fn http_probe_statuses_and_host_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { break };
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let status = if !req.contains("host: web.localhost") {
                    "421 Misdirected"
                } else if req.starts_with("get /ok ") {
                    "200 OK"
                } else if req.starts_with("get /redirect ") {
                    "302 Found\r\nlocation: http://elsewhere.invalid/"
                } else {
                    "500 Oops"
                };
                let _ = s.write_all(format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\n\r\n").as_bytes()).await;
            }
        });
        let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        assert_eq!(http_probe(&client, port, "/ok", "web.localhost").await, Ok(()));
        assert_eq!(http_probe(&client, port, "/redirect", "web.localhost").await, Ok(()), "3xx counts as healthy");
        assert_eq!(
            http_probe(&client, port, "/bad", "web.localhost").await,
            Err(ProbeError::Status("GET /bad returned HTTP 500".to_string()))
        );
        assert!(matches!(
            http_probe(&client, port, "/ok", "other.localhost").await,
            Err(ProbeError::Status(m)) if m.contains("421")
        ));
        server.abort();
        // Nothing listens: a connection problem (which the port hint explains).
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        assert!(matches!(http_probe(&client, port, "/ok", "web.localhost").await, Err(ProbeError::Connection(_))));
    }
}
