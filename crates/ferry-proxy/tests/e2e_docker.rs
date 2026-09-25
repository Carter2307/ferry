//! Docker-gated end-to-end tests: real containers published on
//! `127.0.0.1:<ephemeral>` behind the proxy (the networking model of
//! DESIGN.md §3). Run with `FERRY_E2E=1`; skipped otherwise.

use std::net::SocketAddr;
use std::process::Command;
use std::time::{Duration, Instant};

use ferry_core::CancellationToken;
use ferry_proxy::{ProxyConfig, RouteTable, serve};

fn enabled() -> bool {
    if std::env::var("FERRY_E2E").as_deref() == Ok("1") {
        true
    } else {
        println!("skipped (set FERRY_E2E=1 to run Docker tests)");
        false
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().expect("running docker");
    assert!(out.status.success(), "docker {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every container carries `ferry.instance=<prefix>`; all of them are removed
/// on drop, even when the test panics.
struct Containers {
    prefix: String,
}

impl Containers {
    fn new() -> Self {
        Containers { prefix: format!("ferrytest-proxy-{}", ferry_core::ids::random_secret(8)) }
    }

    /// `docker run -d` publishing `container_port` on 127.0.0.1:<ephemeral>;
    /// returns the published address.
    fn run(&self, name: &str, container_port: u16, image_and_cmd: &[&str]) -> SocketAddr {
        let full_name = format!("{}-{name}", self.prefix);
        let label = format!("ferry.instance={}", self.prefix);
        let publish = format!("127.0.0.1::{container_port}");
        let mut args = vec!["run", "-d", "--name", &full_name, "--label", "ferry.managed=true", "--label", &label];
        args.extend(["-p", &publish]);
        args.extend(image_and_cmd);
        docker(&args);
        let port = docker(&["port", &full_name, &format!("{container_port}/tcp")]);
        // "127.0.0.1:55012" (possibly several lines on some setups).
        port.lines().next().unwrap().parse().unwrap()
    }
}

impl Drop for Containers {
    fn drop(&mut self) {
        let filter = format!("label=ferry.instance={}", self.prefix);
        if let Ok(out) = Command::new("docker").args(["ps", "-aq", "--filter", &filter]).output() {
            let ids = String::from_utf8_lossy(&out.stdout);
            let ids: Vec<&str> = ids.split_whitespace().collect();
            if !ids.is_empty() {
                let _ = Command::new("docker").arg("rm").arg("-f").args(&ids).output();
            }
        }
    }
}

/// Poll until the container answers HTTP (straight, not through the proxy).
async fn wait_http(addr: SocketAddr, path: &str) {
    let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(r) = client.get(format!("http://{addr}{path}")).send().await
            && r.status().is_success()
        {
            return;
        }
        assert!(Instant::now() < deadline, "container at {addr} never became ready");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

struct RunningProxy {
    addr: SocketAddr,
    routes: RouteTable,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<ferry_core::Result<()>>,
}

async fn start_proxy() -> RunningProxy {
    let addr = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let routes = RouteTable::new();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(serve(ProxyConfig::http(addr), routes.clone(), shutdown.clone()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while tokio::net::TcpStream::connect(addr).await.is_err() {
        assert!(Instant::now() < deadline, "proxy never listened");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    RunningProxy { addr, routes, shutdown, task }
}

impl RunningProxy {
    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(12), self.task).await.unwrap().unwrap().unwrap();
    }
}

#[tokio::test]
async fn proxies_to_containers_with_round_robin() {
    if !enabled() {
        return;
    }
    let containers = Containers::new();
    let serve_text =
        |text: &str| format!("echo {text} > /usr/share/nginx/html/index.html && exec nginx -g 'daemon off;'");
    let (one, two) = (serve_text("instance-one"), serve_text("instance-two"));
    let a = containers.run("web-1", 80, &["nginx:alpine", "sh", "-c", &one]);
    let b = containers.run("web-2", 80, &["nginx:alpine", "sh", "-c", &two]);
    wait_http(a, "/").await;
    wait_http(b, "/").await;

    let proxy = start_proxy().await;
    proxy.routes.set_service_routes("srv-web", &["web.localhost".to_string()], vec![a, b]);
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve("web.localhost", "127.0.0.1:0".parse().unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let url = format!("http://web.localhost:{}/", proxy.addr.port());
    let mut bodies = Vec::new();
    for _ in 0..4 {
        let r = client.get(&url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        bodies.push(r.text().await.unwrap().trim().to_string());
    }
    assert_eq!(bodies, ["instance-one", "instance-two", "instance-one", "instance-two"]);

    // A stopped instance: the bodyless GET is retried on the other one.
    docker(&["stop", "-t", "0", &format!("{}-web-1", containers.prefix)]);
    for _ in 0..4 {
        let r = client.get(&url).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.text().await.unwrap().trim(), "instance-two");
    }
    proxy.stop().await;
}

const STREAM_SERVER: &str = r#"
import http.server, time
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_GET(self):
        if self.path == "/headers":
            body = "".join(f"{k.lower()}={v}\n" for k, v in self.headers.items()).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        for i in range(3):
            data = f"data: {i}\n\n".encode()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
            self.wfile.flush()
            time.sleep(1)
        self.wfile.write(b"0\r\n\r\n")
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("0.0.0.0", 8000), H).serve_forever()
"#;

#[tokio::test]
async fn streams_from_a_container_and_forwards_headers() {
    if !enabled() {
        return;
    }
    let containers = Containers::new();
    let app = containers.run("stream", 8000, &["python:3.12-alpine", "python", "-u", "-c", STREAM_SERVER]);
    wait_http(app, "/headers").await;

    let proxy = start_proxy().await;
    proxy.routes.set_service_routes("srv-stream", &["stream.localhost".to_string()], vec![app]);
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve("stream.localhost", "127.0.0.1:0".parse().unwrap())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let base = format!("http://stream.localhost:{}", proxy.addr.port());

    let headers = client.get(format!("{base}/headers")).header("X-Forwarded-For", "203.0.113.9").send().await.unwrap();
    let headers = headers.text().await.unwrap();
    assert!(headers.contains(&format!("host=stream.localhost:{}\n", proxy.addr.port())), "{headers}");
    assert!(headers.contains("x-forwarded-for=203.0.113.9, 127.0.0.1\n"), "{headers}");
    assert!(headers.contains("x-forwarded-proto=http\n"), "{headers}");
    assert!(headers.contains("x-request-id="), "{headers}");

    let started = Instant::now();
    let mut resp = client.get(format!("{base}/events")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let first = resp.chunk().await.unwrap().unwrap();
    let first_after = started.elapsed();
    assert!(String::from_utf8_lossy(&first).starts_with("data: 0"), "{first:?}");
    assert!(first_after < Duration::from_millis(1500), "first event took {first_after:?}: buffered?");
    let mut rest = Vec::new();
    while let Some(chunk) = resp.chunk().await.unwrap() {
        rest.extend_from_slice(&chunk);
    }
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(String::from_utf8_lossy(&rest), "data: 1\n\ndata: 2\n\n");
    proxy.stop().await;
}
