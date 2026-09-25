//! Readiness probes: health checks of new instances and the TCP check that
//! decides which instances receive traffic.

use std::time::{Duration, Instant};

use ferry_core::Service;
use ferry_docker::{ContainerInfo, ContainerState};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::state::Inner;

/// How often a new instance is probed.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
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

/// One HTTP health probe. `Err` carries a short reason.
pub(crate) async fn http_probe(client: &reqwest::Client, port: u16, path: &str, host: &str) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{port}{path}");
    match client.get(&url).header(reqwest::header::HOST, host).send().await {
        Ok(resp) if resp.status().as_u16() < 400 => Ok(()),
        Ok(resp) => Err(format!("GET {path} returned HTTP {}", resp.status().as_u16())),
        Err(e) if e.is_timeout() => Err(format!("GET {path} timed out")),
        Err(e) if e.is_connect() => Err(format!("GET {path}: connection refused")),
        Err(_) => Err(format!("GET {path}: the connection was closed without a response")),
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

/// Probe one new instance every second until it is healthy, it crashes or
/// `deadline` passes. `started` is when it was started. Returns the host
/// port that was verified (`None` for workers).
pub(crate) async fn wait_healthy(
    inner: &Inner,
    container: &ContainerInfo,
    probe: &Probe,
    started: Instant,
    deadline: Instant,
    timeout_secs: u64,
) -> Result<Option<u16>, InstanceFailure> {
    let instance = crate::util::instance_id(&container.name);
    let fail = |message: String, crashed: bool| InstanceFailure { container: container.clone(), message, crashed };
    loop {
        let last: String = match inner.docker.inspect_container(&container.id).await {
            Ok(None) => return Err(fail(format!("instance {instance} was removed"), false)),
            Ok(Some(info)) => match info.state {
                ContainerState::Exited | ContainerState::Dead => {
                    let code = info.exit_code.map(|c| format!(" with code {c}")).unwrap_or_default();
                    return Err(fail(format!("instance {instance} exited{code}"), true));
                }
                ContainerState::Restarting => {
                    return Err(fail(format!("instance {instance} crashed and is being restarted"), true));
                }
                ContainerState::Running if info.restart_count.unwrap_or(0) > 0 => {
                    return Err(fail(format!("instance {instance} crashed and was restarted"), true));
                }
                ContainerState::Running => match probe {
                    Probe::Uptime => {
                        if started.elapsed() >= WORKER_MIN_UPTIME {
                            return Ok(None);
                        }
                        format!("instance {instance} has not been up for {}s yet", WORKER_MIN_UPTIME.as_secs())
                    }
                    Probe::Tcp => match info.host_port.or(container.host_port) {
                        Some(port) if tcp_accepting(port, TCP_SETTLE).await => return Ok(Some(port)),
                        Some(port) => format!("nothing accepts connections on port {port} yet"),
                        None => return Err(fail(format!("instance {instance} has no published port"), false)),
                    },
                    Probe::Http { path, host } => match (info.host_port.or(container.host_port), &inner.http) {
                        (Some(port), Ok(client)) => match http_probe(client, port, path, host).await {
                            Ok(()) => return Ok(Some(port)),
                            Err(reason) => reason,
                        },
                        (None, _) => return Err(fail(format!("instance {instance} has no published port"), false)),
                        (_, Err(e)) => return Err(fail(e.clone(), false)),
                    },
                },
                other => format!("instance {instance} is {}", other.as_str()),
            },
            Err(e) => format!("inspecting instance {instance}: {e}"),
        };
        let now = Instant::now();
        if now >= deadline {
            return Err(fail(format!("health check timed out after {timeout_secs}s: {last}"), false));
        }
        tokio::time::sleep(POLL_INTERVAL.min(deadline - now)).await;
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
            Err("GET /bad returned HTTP 500".to_string())
        );
        assert!(http_probe(&client, port, "/ok", "other.localhost").await.unwrap_err().contains("421"));
        server.abort();
    }
}
