//! Readiness probes: health checks of new instances and the TCP check that
//! decides which instances receive traffic.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ferry_core::{LogSink, Runtime, Service, ServiceType};
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
/// Exit code of a process killed by SIGKILL — the kernel's OOM killer's
/// signal.
pub(crate) const SIGKILL_EXIT_CODE: i64 = 137;
/// How long (polls × interval) a SIGKILLed instance's Docker events are
/// watched for an `oom`, which Docker can record just after the exit.
const OOM_EVENT_POLLS: usize = 10;
const OOM_EVENT_POLL: Duration = Duration::from_millis(200);
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
    /// It exited with code 0: nothing failed, its command just ended. What
    /// ran, and what to run instead, is worth saying (see [`clean_exit`]).
    pub clean_exit: bool,
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

/// Where the command of an instance comes from: what there is to change
/// when it is not the right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandSource {
    /// The service's start command.
    Service,
    /// The image's own command (the project's Dockerfile, a prebuilt image).
    Image,
    /// Picked from the project, for a service without a start command (the
    /// build log's `==> Start command` line says how).
    Project,
}

impl CommandSource {
    /// `runtime`: what the deploy's image was built for, when known.
    pub(crate) fn of(svc: &Service, runtime: Option<Runtime>) -> Self {
        if svc.start_command.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            CommandSource::Service
        } else if matches!(runtime, Some(Runtime::Docker | Runtime::Image)) {
            CommandSource::Image
        } else {
            CommandSource::Project
        }
    }
}

/// A failure, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Explained {
    pub message: String,
    pub hint: String,
}

fn service_kind(t: ServiceType) -> &'static str {
    match t {
        ServiceType::WebService => "web service",
        ServiceType::PrivateService => "private service",
        ServiceType::BackgroundWorker => "background worker",
        ServiceType::CronJob => "cron job",
        ServiceType::StaticSite => "static site",
    }
}

/// An instance whose command ended without an error (exit code 0) before it
/// was healthy. Nothing crashed and nothing was printed, so the exit code
/// alone explains nothing: say what ran (`command`, when Docker told), why
/// ending is a failure for this kind of service, and what to change.
pub(crate) fn clean_exit(
    instance: &str,
    command: Option<&str>,
    svc: &Service,
    port: Option<u16>,
    source: CommandSource,
) -> Explained {
    let worker = svc.service_type == ServiceType::BackgroundWorker;
    let ran = match command {
        Some(c) => format!("its command `{c}` ended"),
        None => "its command ended".to_string(),
    };
    let kind = service_kind(svc.service_type);
    let must = match port {
        Some(p) if !worker => format!("a {kind} must keep running and listen on port {p}"),
        _ => format!("a {kind} must keep running"),
    };
    let message = format!("instance {instance} exited with code 0: {ran} without an error, but {must}");

    let one_off = "a task that runs and ends belongs in a cron job or a one-off job (`ferry run`)";
    let hint = match (source, worker) {
        (CommandSource::Project, false) => {
            "the service has no start command, so Ferry picked this one from the project (see \"==> Start command\" \
             in the build log). If the project is an app, set the start command that runs its server. If it only \
             builds files (a static site, a component library), create it as a static site instead."
                .to_string()
        }
        (CommandSource::Project, true) => format!(
            "the service has no start command, so Ferry picked this one from the project (see \"==> Start \
             command\" in the build log). Set a start command that keeps running; {one_off}."
        ),
        (CommandSource::Service, false) => {
            "this is the service's start command: it must start a server and stay in the foreground. Steps that \
             only build belong in the build command, and a project that only builds files (a static site, a \
             component library) is deployed as a static site."
                .to_string()
        }
        (CommandSource::Service, true) => {
            format!("this is the service's start command: it must keep running; {one_off}.")
        }
        (CommandSource::Image, false) => {
            "this is the image's own command (its CMD or ENTRYPOINT): it must start a server and stay in the \
             foreground. Set a start command on the service to run something else."
                .to_string()
        }
        (CommandSource::Image, true) => format!(
            "this is the image's own command (its CMD or ENTRYPOINT): it must keep running. Set a start command \
             on the service to run something else; {one_off}."
        ),
    };
    Explained { message, hint }
}

/// What a container runs, as it would be typed, from what Docker reports:
/// its command (`cmd`), else the whole process (`path` and `args`: images
/// that only have an entrypoint). A shell wrapper (`/bin/sh -c …`, how start
/// commands are run) shows as its script.
pub(crate) fn display_command(cmd: &[String], path: Option<&str>, args: &[String]) -> Option<String> {
    /// Long enough for a real command, short enough for one log line.
    const MAX_CHARS: usize = 200;
    let words: Vec<&str> = if cmd.is_empty() {
        path.into_iter().chain(args.iter().map(String::as_str)).collect()
    } else {
        cmd.iter().map(String::as_str).collect()
    };
    let shown = match words.as_slice() {
        [shell, "-c", script] if matches!(*shell, "/bin/sh" | "sh" | "/bin/bash" | "bash") => script.to_string(),
        other => other.join(" "),
    };
    // One line: a script's line breaks would cut the message in two.
    let shown = shown.split_whitespace().collect::<Vec<_>>().join(" ");
    if shown.is_empty() {
        return None;
    }
    Some(match shown.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &shown[..cut]),
        None => shown,
    })
}

/// The command instance `id` runs, when Docker can tell.
pub(crate) async fn container_command(inner: &Inner, id: &str) -> Option<String> {
    let resp = inner.docker.bollard().inspect_container(id, None).await.ok()?;
    let cmd = resp.config.and_then(|c| c.cmd).unwrap_or_default();
    display_command(&cmd, resp.path.as_deref(), &resp.args.unwrap_or_default())
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

/// `exit`, or an OOM kill Docker recorded late. Only a SIGKILL can be the
/// kernel's OOM killer, and Docker may record the OOM kill a moment after it
/// reports the exit (seen on Linux hosts): an instance (or a job) SIGKILLed
/// with no OOM kill on record is looked up in its Docker events since `since`
/// (a time since the epoch) a while longer.
pub(crate) async fn with_late_oom(inner: &Inner, id: &str, since: Duration, exit: LastExit) -> LastExit {
    if exit.oom_killed || exit.code != Some(SIGKILL_EXIT_CODE) {
        return exit;
    }
    for attempt in 0..OOM_EVENT_POLLS {
        if exit_from_events(inner, id, since, exit.memory_limit_bytes).await.oom_killed {
            return LastExit { oom_killed: true, ..exit };
        }
        if attempt + 1 < OOM_EVENT_POLLS {
            tokio::time::sleep(OOM_EVENT_POLL).await;
        }
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
    let fail =
        |message: String| InstanceFailure { container: container.clone(), message, crashed: false, clean_exit: false };
    // The process ended: out of memory, with an error, or without one.
    let exited = |exit: LastExit| InstanceFailure {
        container: container.clone(),
        message: crash_message(&instance, exit, check.worker),
        crashed: true,
        clean_exit: !exit.oom_killed && exit.code == Some(0),
    };
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
            Ok(None) => return Err(fail(format!("instance {instance} was removed"))),
            Ok(Some(info)) => match info.state {
                ContainerState::Exited | ContainerState::Dead => {
                    let exit = LastExit {
                        code: info.exit_code,
                        oom_killed: info.oom_killed,
                        memory_limit_bytes: info.memory_limit_bytes,
                    };
                    let exit = with_late_oom(inner, &container.id, started_at, exit).await;
                    return Err(exited(exit));
                }
                ContainerState::Restarting => {
                    let exit = last_exit(inner, &container.id, started_at, info.memory_limit_bytes).await;
                    let exit = with_late_oom(inner, &container.id, started_at, exit).await;
                    return Err(exited(exit));
                }
                ContainerState::Running if info.restart_count.unwrap_or(0) > 0 => {
                    let exit = last_exit(inner, &container.id, started_at, info.memory_limit_bytes).await;
                    let exit = with_late_oom(inner, &container.id, started_at, exit).await;
                    return Err(exited(exit));
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
                        None => return Err(fail(format!("instance {instance} has no published port"))),
                    },
                    Probe::Http { path, host } => match (info.host_port.or(container.host_port), &inner.http) {
                        (Some(port), Ok(client)) => match http_probe(client, port, path, host).await {
                            Ok(()) => return Ok(Some(port)),
                            Err(ProbeError::Status(reason)) => (reason, false),
                            Err(ProbeError::Connection(reason)) => (format!("{reason} on {}", port_text(port)), true),
                        },
                        (None, _) => return Err(fail(format!("instance {instance} has no published port"))),
                        (_, Err(e)) => return Err(fail(e.clone())),
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
            return Err(fail(format!("health check timed out after {}s: {last}{hint}", check.timeout_secs)));
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

    /// What a library deployed as a web service looks like: no start command,
    /// so its "main" file is run, which ends at once without printing anything.
    #[test]
    fn clean_exits_say_what_ran_and_what_to_change() {
        let web = Service::new("libui", ServiceType::WebService);
        let source = CommandSource::of(&web, Some(Runtime::Node));
        assert_eq!(source, CommandSource::Project);
        let e = clean_exit("2e58a7", Some("node ./dist/index.js"), &web, Some(10000), source);
        assert_eq!(
            e.message,
            "instance 2e58a7 exited with code 0: its command `node ./dist/index.js` ended without an error, but a \
             web service must keep running and listen on port 10000"
        );
        assert_eq!(
            e.hint,
            "the service has no start command, so Ferry picked this one from the project (see \"==> Start \
             command\" in the build log). If the project is an app, set the start command that runs its server. If \
             it only builds files (a static site, a component library), create it as a static site instead."
        );

        // The service's own start command: it is the one to fix.
        let mut own = Service::new("api", ServiceType::PrivateService);
        own.start_command = Some(" npm run build ".into());
        let source = CommandSource::of(&own, Some(Runtime::Node));
        assert_eq!(source, CommandSource::Service);
        let e = clean_exit("ab12cd", Some("npm run build"), &own, Some(3000), source);
        assert!(e.message.ends_with("but a private service must keep running and listen on port 3000"), "{e:?}");
        assert!(e.hint.starts_with("this is the service's start command: it must start a server"), "{e:?}");
        assert!(e.hint.contains("build command"), "{e:?}");

        // The image's command (a Dockerfile of the project, a prebuilt image).
        for runtime in [Runtime::Docker, Runtime::Image] {
            let source = CommandSource::of(&web, Some(runtime));
            assert_eq!(source, CommandSource::Image);
            let e = clean_exit("ab12cd", None, &web, Some(80), source);
            assert_eq!(
                e.message,
                "instance ab12cd exited with code 0: its command ended without an error, but a web service must \
                 keep running and listen on port 80"
            );
            assert!(e.hint.starts_with("this is the image's own command (its CMD or ENTRYPOINT)"), "{e:?}");
            assert!(e.hint.ends_with("Set a start command on the service to run something else."), "{e:?}");
        }
        // The runtime is not known (the build's record is gone): the project's.
        assert_eq!(CommandSource::of(&web, None), CommandSource::Project);
        let mut blank = web.clone();
        blank.start_command = Some("   ".into());
        assert_eq!(CommandSource::of(&blank, Some(Runtime::Docker)), CommandSource::Image);

        // Workers listen on nothing; what runs and ends is a job.
        let worker = Service::new("mailer", ServiceType::BackgroundWorker);
        for (source, start) in [
            (CommandSource::Project, "the service has no start command, so Ferry picked this one"),
            (CommandSource::Service, "this is the service's start command: it must keep running"),
            (CommandSource::Image, "this is the image's own command (its CMD or ENTRYPOINT): it must keep running"),
        ] {
            let e = clean_exit("ab12cd", Some("python send.py"), &worker, None, source);
            assert_eq!(
                e.message,
                "instance ab12cd exited with code 0: its command `python send.py` ended without an error, but a \
                 background worker must keep running"
            );
            assert!(e.hint.starts_with(start), "{e:?}");
            assert!(e.hint.ends_with("belongs in a cron job or a one-off job (`ferry run`)."), "{e:?}");
        }
    }

    #[test]
    fn commands_are_shown_as_they_would_be_typed() {
        let words = |w: &[&str]| -> Vec<String> { w.iter().map(|s| s.to_string()).collect() };
        // What a generated Dockerfile's CMD is, behind the image's entrypoint.
        assert_eq!(
            display_command(&words(&["node", "./dist/index.js"]), Some("docker-entrypoint.sh"), &words(&["node"])),
            Some("node ./dist/index.js".to_string())
        );
        // A start command is run by a shell: its script is what was typed.
        for shell in ["/bin/sh", "sh", "/bin/bash", "bash"] {
            assert_eq!(
                display_command(&words(&[shell, "-c", " gunicorn app:app -b 0.0.0.0:$PORT "]), None, &[]),
                Some("gunicorn app:app -b 0.0.0.0:$PORT".to_string())
            );
        }
        // No command: the entrypoint and its arguments.
        assert_eq!(
            display_command(&[], Some("/usr/local/bin/app"), &words(&["--once"])),
            Some("/usr/local/bin/app --once".to_string())
        );
        assert_eq!(display_command(&[], None, &[]), None);
        assert_eq!(display_command(&words(&["/bin/sh", "-c", "  "]), None, &[]), None);
        // One line, of a bounded length.
        assert_eq!(
            display_command(&words(&["/bin/sh", "-c", "set -e\nmigrate\n  serve"]), None, &[]),
            Some("set -e migrate serve".to_string())
        );
        let long = display_command(&words(&["echo", &"é".repeat(500)]), None, &[]).unwrap();
        assert_eq!(long.chars().count(), 201, "{long}");
        assert!(long.ends_with('…'), "{long}");
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
