//! Conversions between Ferry's types and bollard's API models.

use std::collections::{BTreeMap, HashMap};

use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, ContainerSummary, EndpointSettings,
    HostConfig, Mount, MountType, NetworkingConfig, PortBinding, PortMap, PortSummary, PortSummaryTypeEnum,
    RestartPolicy as ApiRestartPolicy, RestartPolicyNameEnum,
};

use crate::{ContainerInfo, ContainerSpec, ContainerState, RestartPolicy};

/// `"<port>/tcp"`, the key Docker uses for exposed ports and port maps.
pub(crate) fn tcp_port_key(port: u16) -> String {
    format!("{port}/tcp")
}

/// Parse a port key (`80/tcp`, `53/udp`, `8080`) into (port, protocol).
fn parse_port_key(key: &str) -> Option<(u16, &str)> {
    let (port, proto) = key.split_once('/').unwrap_or((key, "tcp"));
    let port = port.trim().parse::<u16>().ok().filter(|p| *p > 0)?;
    Some((port, proto.trim()))
}

/// TCP ports of an image's `ExposedPorts` keys, sorted ascending and deduplicated.
pub(crate) fn exposed_tcp_ports<S: AsRef<str>>(keys: &[S]) -> Vec<u16> {
    let mut ports: Vec<u16> = keys
        .iter()
        .filter_map(|k| parse_port_key(k.as_ref()))
        .filter(|(_, proto)| proto.eq_ignore_ascii_case("tcp"))
        .map(|(port, _)| port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn restart_policy(policy: RestartPolicy) -> ApiRestartPolicy {
    let name = match policy {
        RestartPolicy::No => RestartPolicyNameEnum::NO,
        RestartPolicy::UnlessStopped => RestartPolicyNameEnum::UNLESS_STOPPED,
        RestartPolicy::OnFailure => RestartPolicyNameEnum::ON_FAILURE,
    };
    ApiRestartPolicy { name: Some(name), maximum_retry_count: None }
}

fn to_hash_map(labels: &BTreeMap<String, String>) -> HashMap<String, String> {
    labels.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

pub(crate) fn labels_to_api(labels: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    (!labels.is_empty()).then(|| to_hash_map(labels))
}

/// The body of `POST /containers/create` for a spec.
pub(crate) fn create_body(spec: &ContainerSpec) -> ContainerCreateBody {
    let mut host_config = HostConfig {
        restart_policy: Some(restart_policy(spec.restart_policy)),
        memory: spec.memory_limit_bytes.filter(|m| *m > 0),
        nano_cpus: spec.nano_cpus.filter(|n| *n > 0),
        ..Default::default()
    };
    if !spec.volumes.is_empty() {
        host_config.mounts = Some(
            spec.volumes
                .iter()
                .map(|v| Mount {
                    target: Some(v.target.clone()),
                    source: Some(v.volume.clone()),
                    typ: Some(MountType::VOLUME),
                    ..Default::default()
                })
                .collect(),
        );
    }

    let mut exposed_ports = None;
    if let Some(publish) = &spec.publish {
        let key = tcp_port_key(publish.container_port);
        let binding = PortBinding {
            host_ip: Some(publish.host_ip.clone()),
            // An empty host port asks Docker for an ephemeral one.
            host_port: Some(publish.host_port.map(|p| p.to_string()).unwrap_or_default()),
        };
        host_config.port_bindings = Some(HashMap::from([(key.clone(), Some(vec![binding]))]));
        exposed_ports = Some(vec![key]);
    }

    let mut networking_config = None;
    if let Some(network) = spec.network.as_deref().filter(|n| !n.is_empty()) {
        // Primary network = ours (not the default bridge), like `docker run --network`.
        host_config.network_mode = Some(network.to_string());
        let endpoint = EndpointSettings {
            aliases: (!spec.network_aliases.is_empty()).then(|| spec.network_aliases.clone()),
            ..Default::default()
        };
        networking_config =
            Some(NetworkingConfig { endpoints_config: Some(HashMap::from([(network.to_string(), endpoint)])) });
    }

    ContainerCreateBody {
        image: Some(spec.image.clone()),
        env: (!spec.env.is_empty()).then(|| spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        cmd: spec.cmd.clone(),
        entrypoint: spec.entrypoint.clone(),
        labels: labels_to_api(&spec.labels),
        working_dir: spec.working_dir.clone().filter(|w| !w.is_empty()),
        exposed_ports,
        host_config: Some(host_config),
        networking_config,
        ..Default::default()
    }
}

/// First usable host port of a binding list, preferring IPv4 bindings
/// (Docker lists `0.0.0.0` and `::` separately when binding all interfaces).
fn binding_port(bindings: &[PortBinding]) -> Option<u16> {
    let port = |b: &PortBinding| b.host_port.as_deref().and_then(|p| p.trim().parse::<u16>().ok()).filter(|p| *p > 0);
    let is_ipv6 = |b: &PortBinding| b.host_ip.as_deref().unwrap_or("").contains(':');
    bindings.iter().filter(|b| !is_ipv6(b)).find_map(port).or_else(|| bindings.iter().find_map(port))
}

/// Host port bound for the lowest published TCP container port of an
/// inspect port map (`NetworkSettings.Ports`).
pub(crate) fn host_port_from_map(ports: &PortMap) -> Option<u16> {
    let mut keys: Vec<(u16, &String)> = ports
        .keys()
        .filter_map(|k| parse_port_key(k).filter(|(_, proto)| proto.eq_ignore_ascii_case("tcp")).map(|(p, _)| (p, k)))
        .collect();
    keys.sort_unstable();
    keys.into_iter().find_map(|(_, key)| ports.get(key).and_then(Option::as_deref).and_then(binding_port))
}

/// Same as [`host_port_from_map`] for the port list of `GET /containers/json`.
pub(crate) fn host_port_from_summary(ports: &[PortSummary]) -> Option<u16> {
    let mut tcp: Vec<&PortSummary> = ports
        .iter()
        .filter(|p| matches!(p.typ, None | Some(PortSummaryTypeEnum::TCP) | Some(PortSummaryTypeEnum::EMPTY)))
        .filter(|p| p.public_port.is_some_and(|port| port > 0))
        .collect();
    tcp.sort_by_key(|p| (p.private_port, p.ip.as_deref().unwrap_or("").contains(':')));
    tcp.first().and_then(|p| p.public_port)
}

/// Exit code from a list status string such as `Exited (137) 5 seconds ago`.
pub(crate) fn exit_code_from_status(status: &str) -> Option<i64> {
    let rest = status.trim().strip_prefix("Exited (")?;
    let end = rest.find(')')?;
    rest[..end].trim().parse().ok()
}

fn state_from_inspect(state: &bollard::models::ContainerState) -> ContainerState {
    match state.status {
        Some(status) if status != ContainerStateStatusEnum::EMPTY => ContainerState::parse(status.as_ref()),
        _ if state.dead == Some(true) => ContainerState::Dead,
        _ if state.restarting == Some(true) => ContainerState::Restarting,
        _ if state.paused == Some(true) => ContainerState::Paused,
        _ if state.running == Some(true) => ContainerState::Running,
        _ => ContainerState::Unknown,
    }
}

/// A container name without Docker's leading `/`. Linked containers also get
/// names like `/other/alias`; the container's own name has no inner `/`.
fn primary_name(names: &[String]) -> String {
    let trimmed = |n: &String| n.trim_start_matches('/').to_string();
    names.iter().map(trimmed).find(|n| !n.contains('/')).or_else(|| names.first().map(trimmed)).unwrap_or_default()
}

/// A timestamp Docker reports for containers that never started.
fn is_zero_time(ts: &str) -> bool {
    ts.is_empty() || ts.starts_with("0001-01-01")
}

pub(crate) fn info_from_inspect(resp: ContainerInspectResponse) -> ContainerInfo {
    let api_state = resp.state.as_ref();
    let state = api_state.map_or(ContainerState::Unknown, state_from_inspect);
    let config = resp.config.as_ref();
    let exited = matches!(state, ContainerState::Exited | ContainerState::Dead);
    ContainerInfo {
        id: resp.id.clone().unwrap_or_default(),
        name: resp.name.as_deref().map(|n| n.trim_start_matches('/').to_string()).unwrap_or_default(),
        // The reference the container was created from (not the image id).
        image: config.and_then(|c| c.image.clone()).or_else(|| resp.image.clone()).unwrap_or_default(),
        state,
        exit_code: if exited { api_state.and_then(|s| s.exit_code) } else { None },
        host_port: resp.network_settings.as_ref().and_then(|n| n.ports.as_ref()).and_then(host_port_from_map),
        labels: config.and_then(|c| c.labels.as_ref()).map(|l| l.clone().into_iter().collect()).unwrap_or_default(),
        started_at: if state.is_running() {
            api_state.and_then(|s| s.started_at.clone()).filter(|t| !is_zero_time(t))
        } else {
            None
        },
        restart_count: resp.restart_count,
    }
}

pub(crate) fn info_from_summary(summary: ContainerSummary) -> ContainerInfo {
    let state = summary.state.map_or(ContainerState::Unknown, |s| ContainerState::parse(s.as_ref()));
    let exited = matches!(state, ContainerState::Exited | ContainerState::Dead);
    ContainerInfo {
        id: summary.id.unwrap_or_default(),
        name: primary_name(summary.names.as_deref().unwrap_or_default()),
        image: summary.image.unwrap_or_default(),
        state,
        exit_code: if exited { summary.status.as_deref().and_then(exit_code_from_status) } else { None },
        host_port: summary.ports.as_deref().and_then(host_port_from_summary),
        labels: summary.labels.map(|l| l.into_iter().collect()).unwrap_or_default(),
        started_at: None,
        restart_count: None,
    }
}

#[cfg(test)]
mod tests {
    use bollard::models::{ContainerConfig, ContainerSummaryStateEnum, NetworkSettings};
    use serde_json::json;

    use super::*;
    use crate::{PortPublish, VolumeMount};

    fn spec() -> ContainerSpec {
        ContainerSpec {
            name: "ferry-web-1".into(),
            image: "busybox:stable".into(),
            env: vec![("PORT".into(), "8080".into()), ("EMPTY".into(), String::new())],
            cmd: Some(vec!["httpd".into(), "-f".into()]),
            entrypoint: None,
            labels: BTreeMap::from([("ferry.managed".into(), "true".into())]),
            network: Some("ferry".into()),
            network_aliases: vec!["web".into()],
            publish: Some(PortPublish { container_port: 8080, host_ip: "127.0.0.1".into(), host_port: None }),
            volumes: vec![VolumeMount { volume: "ferry-svc-disk".into(), target: "/data".into() }],
            restart_policy: RestartPolicy::UnlessStopped,
            memory_limit_bytes: Some(64 << 20),
            nano_cpus: Some(500_000_000),
            working_dir: Some("/srv".into()),
        }
    }

    #[test]
    fn create_body_json() {
        let body = serde_json::to_value(create_body(&spec())).unwrap();
        assert_eq!(body["Image"], "busybox:stable");
        assert_eq!(body["Env"], json!(["PORT=8080", "EMPTY="]));
        assert_eq!(body["Cmd"], json!(["httpd", "-f"]));
        assert!(body.get("Entrypoint").is_none());
        assert_eq!(body["Labels"], json!({"ferry.managed": "true"}));
        assert_eq!(body["WorkingDir"], "/srv");
        assert_eq!(body["ExposedPorts"], json!({"8080/tcp": {}}));
        let hc = &body["HostConfig"];
        assert_eq!(hc["RestartPolicy"]["Name"], "unless-stopped");
        assert_eq!(hc["Memory"], 64 << 20);
        assert_eq!(hc["NanoCpus"], 500_000_000);
        assert_eq!(hc["NetworkMode"], "ferry");
        assert_eq!(hc["PortBindings"], json!({"8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": ""}]}));
        assert_eq!(hc["Mounts"], json!([{"Target": "/data", "Source": "ferry-svc-disk", "Type": "volume"}]));
        assert_eq!(body["NetworkingConfig"]["EndpointsConfig"]["ferry"]["Aliases"], json!(["web"]));
    }

    #[test]
    fn create_body_minimal() {
        let minimal = ContainerSpec { name: "job".into(), image: "alpine:3.20".into(), ..Default::default() };
        let body = serde_json::to_value(create_body(&minimal)).unwrap();
        assert_eq!(body["HostConfig"]["RestartPolicy"]["Name"], "no");
        for absent in ["Env", "Labels", "ExposedPorts", "NetworkingConfig", "Cmd", "WorkingDir"] {
            assert!(body.get(absent).is_none(), "{absent} should be absent: {body}");
        }
        for absent in ["PortBindings", "Mounts", "NetworkMode", "Memory", "NanoCpus"] {
            assert!(body["HostConfig"].get(absent).is_none(), "{absent} should be absent");
        }

        let fixed = ContainerSpec {
            publish: Some(PortPublish { container_port: 5432, host_ip: "0.0.0.0".into(), host_port: Some(15432) }),
            entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]),
            restart_policy: RestartPolicy::OnFailure,
            ..minimal
        };
        let body = serde_json::to_value(create_body(&fixed)).unwrap();
        assert_eq!(body["HostConfig"]["PortBindings"]["5432/tcp"][0]["HostPort"], "15432");
        assert_eq!(body["HostConfig"]["RestartPolicy"]["Name"], "on-failure");
        assert_eq!(body["Entrypoint"], json!(["/bin/sh", "-c"]));
    }

    #[test]
    fn exposed_ports() {
        assert_eq!(exposed_tcp_ports(&["80/tcp"]), vec![80]);
        assert_eq!(
            exposed_tcp_ports(&["8080/tcp", "53/udp", "443/TCP", "80/tcp", "80/tcp", "9000"]),
            vec![80, 443, 8080, 9000]
        );
        assert_eq!(exposed_tcp_ports(&["0/tcp", "x/tcp", "70000/tcp", "8000-8010/tcp"]), Vec::<u16>::new());
        assert!(exposed_tcp_ports::<&str>(&[]).is_empty());
    }

    fn binding(ip: &str, port: &str) -> PortBinding {
        PortBinding { host_ip: Some(ip.into()), host_port: Some(port.into()) }
    }

    #[test]
    fn host_ports_from_inspect_map() {
        let mut ports: PortMap = HashMap::new();
        ports.insert("53/udp".into(), Some(vec![binding("0.0.0.0", "5353")]));
        ports.insert("9000/tcp".into(), None);
        assert_eq!(host_port_from_map(&ports), None);
        ports.insert("8080/tcp".into(), Some(vec![binding("::", "49154"), binding("0.0.0.0", "49153")]));
        assert_eq!(host_port_from_map(&ports), Some(49153));
        ports.insert("80/tcp".into(), Some(vec![binding("127.0.0.1", "")]));
        assert_eq!(host_port_from_map(&ports), Some(49153), "unparseable bindings are skipped");
        ports.insert("81/tcp".into(), Some(vec![binding("::1", "40000")]));
        assert_eq!(host_port_from_map(&ports), Some(40000), "lowest container port wins, IPv6 as a fallback");
    }

    fn port(private: u16, public: Option<u16>, ip: &str, typ: PortSummaryTypeEnum) -> PortSummary {
        PortSummary { ip: Some(ip.into()), private_port: private, public_port: public, typ: Some(typ) }
    }

    #[test]
    fn host_ports_from_summary() {
        assert_eq!(host_port_from_summary(&[]), None);
        let ports = vec![
            port(53, Some(5353), "0.0.0.0", PortSummaryTypeEnum::UDP),
            port(8080, Some(49160), "::", PortSummaryTypeEnum::TCP),
            port(8080, Some(49159), "0.0.0.0", PortSummaryTypeEnum::TCP),
            port(9000, None, "", PortSummaryTypeEnum::TCP),
        ];
        assert_eq!(host_port_from_summary(&ports), Some(49159));
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code_from_status("Exited (3) 2 seconds ago"), Some(3));
        assert_eq!(exit_code_from_status("Exited (137) About a minute ago"), Some(137));
        assert_eq!(exit_code_from_status("Up 5 minutes"), None);
        assert_eq!(exit_code_from_status("Exited (x)"), None);
        assert_eq!(exit_code_from_status("Created"), None);
    }

    #[test]
    fn names() {
        assert_eq!(primary_name(&["/web".into()]), "web");
        assert_eq!(primary_name(&["/other/alias".into(), "/web".into()]), "web");
        assert_eq!(primary_name(&["/a/b".into()]), "a/b");
        assert_eq!(primary_name(&[]), "");
    }

    #[test]
    fn inspect_conversion() {
        let running: ContainerInspectResponse = serde_json::from_value(json!({
            "Id": "abc123",
            "Name": "/ferry-web-1",
            "Image": "sha256:deadbeef",
            "RestartCount": 2,
            "State": {"Status": "running", "Running": true, "ExitCode": 0, "StartedAt": "2026-09-25T10:00:00.1Z"},
            "Config": {"Image": "busybox:stable", "Labels": {"a": "b"}},
            "NetworkSettings": {"Ports": {"8080/tcp": [{"HostIp": "127.0.0.1", "HostPort": "49153"}]}}
        }))
        .unwrap();
        let info = info_from_inspect(running);
        assert_eq!(info.id, "abc123");
        assert_eq!(info.name, "ferry-web-1");
        assert_eq!(info.image, "busybox:stable");
        assert_eq!(info.state, ContainerState::Running);
        assert_eq!(info.exit_code, None);
        assert_eq!(info.host_port, Some(49153));
        assert_eq!(info.labels, BTreeMap::from([("a".to_string(), "b".to_string())]));
        assert_eq!(info.started_at.as_deref(), Some("2026-09-25T10:00:00.1Z"));
        assert_eq!(info.restart_count, Some(2));

        let exited = ContainerInspectResponse {
            id: Some("x".into()),
            name: Some("/job".into()),
            image: Some("sha256:1".into()),
            state: Some(bollard::models::ContainerState {
                status: Some(ContainerStateStatusEnum::EXITED),
                exit_code: Some(3),
                started_at: Some("2026-09-25T10:00:00Z".into()),
                ..Default::default()
            }),
            config: Some(ContainerConfig::default()),
            network_settings: Some(NetworkSettings { ports: Some(HashMap::new()), ..Default::default() }),
            ..Default::default()
        };
        let info = info_from_inspect(exited);
        assert_eq!(info.state, ContainerState::Exited);
        assert_eq!(info.exit_code, Some(3));
        assert_eq!(info.image, "sha256:1", "falls back to the image id");
        assert_eq!(info.started_at, None, "only running containers report started_at");
        assert_eq!(info.host_port, None);

        let flags_only = bollard::models::ContainerState { running: Some(true), ..Default::default() };
        assert_eq!(state_from_inspect(&flags_only), ContainerState::Running);
        assert_eq!(state_from_inspect(&Default::default()), ContainerState::Unknown);
        assert!(is_zero_time("0001-01-01T00:00:00Z"));
    }

    #[test]
    fn summary_conversion() {
        let summary = ContainerSummary {
            id: Some("abc".into()),
            names: Some(vec!["/ferry-job-1".into()]),
            image: Some("alpine:3.20".into()),
            state: Some(ContainerSummaryStateEnum::EXITED),
            status: Some("Exited (3) 1 second ago".into()),
            labels: Some(HashMap::from([("ferry.role".into(), "job".into())])),
            ..Default::default()
        };
        let info = info_from_summary(summary);
        assert_eq!(info.name, "ferry-job-1");
        assert_eq!(info.state, ContainerState::Exited);
        assert_eq!(info.exit_code, Some(3));
        assert_eq!(info.labels["ferry.role"], "job");
        assert_eq!(info.host_port, None);
        assert_eq!(info.restart_count, None);

        let running = ContainerSummary {
            state: Some(ContainerSummaryStateEnum::RUNNING),
            status: Some("Up 3 seconds".into()),
            ports: Some(vec![port(80, Some(49999), "127.0.0.1", PortSummaryTypeEnum::TCP)]),
            ..Default::default()
        };
        let info = info_from_summary(running);
        assert_eq!(info.state, ContainerState::Running);
        assert_eq!(info.exit_code, None);
        assert_eq!(info.host_port, Some(49999));
    }
}
