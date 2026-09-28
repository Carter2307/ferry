//! `ferry login` and `ferry info`.

use anyhow::{Context, Result, anyhow};
use ferry_core::dto::ServerInfo;

use super::{Ctx, DefaultLimits, print_json};
use crate::client::{Client, Json};
use crate::config::{self, FileConfig, Settings, Source};
use crate::output::{self, Cell, Color, errln, outln};

/// Verify the resolved server + token with `GET /api/v1/info` and save them.
pub async fn login(settings: &Settings, json: bool) -> Result<()> {
    let token = settings.token.as_deref().ok_or_else(|| {
        let hint = "ferryd prints the token at startup and stores it in <data-dir>/api_token";
        match &settings.saved_login_server {
            // Never reuse the token saved for another server.
            Some(saved) => anyhow!(
                "missing token for {}: the saved token belongs to {saved} and is only sent there; \
                 run 'ferry login --server {} --token <TOKEN>' ({hint})",
                settings.server,
                settings.server
            ),
            None => anyhow!("missing token: run 'ferry login --server <URL> --token <TOKEN>' ({hint})"),
        }
    })?;
    let client = Client::new(&settings.server, token)?;
    let info = client.get::<ServerInfo>(&["info"], &[]).await?;

    let path = config::config_path()
        .ok_or_else(|| anyhow!("cannot locate the config directory: set HOME or XDG_CONFIG_HOME"))?;
    let cfg = FileConfig { server: Some(settings.server.clone()), token: Some(token.to_string()) };
    let save_path = path.clone();
    tokio::task::spawn_blocking(move || config::save(&save_path, &cfg)).await.context("saving config")??;

    if json {
        return print_json(&info.raw);
    }
    outln!("{} {} (Ferry {})", output::out(Color::Green, "Logged in to"), settings.server, info.data.version)?;
    outln!("Saved to {}", path.display())?;
    let resources = resource_pairs(&info);
    if !resources.is_empty() {
        outln!("{}", output::render_kv(&resources, output::stdout_color()).trim_end())?;
    }
    if settings.server_source == Source::Default {
        errln!("note: no --server given, using the default {}", config::DEFAULT_SERVER);
    }
    Ok(())
}

/// `512 MiB memory, 1 CPU per container`, when the server reports its
/// default limits (0 = unlimited).
fn default_limits(d: &DefaultLimits) -> Option<String> {
    let (memory, cpus) = (d.memory_mb?, d.cpus?);
    if memory == 0 && cpus <= 0.0 {
        return Some("none (containers are unlimited)".to_string());
    }
    let cpus = if cpus > 0.0 { output::cpus_or_unlimited(cpus) } else { "unlimited CPU".to_string() };
    Some(format!("{} memory, {cpus} per container", output::memory_or_unlimited(memory)))
}

/// `8 CPUs, 15.6 GiB memory`: the Docker host's capacity, when known.
fn docker_host(i: &ServerInfo) -> Option<String> {
    let cpus = i.docker_cpus.map(|c| if c == 1 { "1 CPU".to_string() } else { format!("{c} CPUs") });
    let memory = i.docker_memory_bytes.map(|m| format!("{} memory", output::human_bytes(m)));
    let parts: Vec<String> = [cpus, memory].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// Key/value lines of `ferry info` about container resources.
fn resource_pairs(info: &Json<ServerInfo>) -> Vec<(&'static str, Cell)> {
    let mut pairs = Vec::new();
    if let Some(l) = default_limits(&DefaultLimits::from_info(info)) {
        pairs.push(("Default limits", Cell::new(l)));
    }
    if let Some(h) = docker_host(&info.data) {
        pairs.push(("Docker host", Cell::new(h)));
    }
    pairs
}

pub async fn info(ctx: &Ctx) -> Result<()> {
    let info = ctx.client.get::<ServerInfo>(&["info"], &[]).await?;
    if ctx.json {
        return print_json(&info.raw);
    }
    let i = &info.data;
    let on_off = |b: bool| Cell::from(if b { "enabled" } else { "disabled" });
    let docker = match &i.docker_version {
        Some(v) => Cell::new(v.as_str()),
        None => Cell::colored("unreachable", Some(Color::Red)),
    };
    let mut pairs = vec![
        ("Server", Cell::new(ctx.client.server())),
        ("Version", Cell::new(format!("{} (CLI {})", i.version, env!("CARGO_PKG_VERSION")))),
        ("Proxy", Cell::new(i.proxy_url.as_str())),
        ("Base domain", Cell::new(i.base_domain.as_str())),
        ("Apps", Cell::new(format!("<name>.{}", i.base_domain))),
        ("TLS", on_off(i.tls_enabled)),
        ("Dashboard", Cell::new(i.dashboard_url.clone().unwrap_or_else(|| ctx.client.server().to_string()))),
        ("GitHub webhook", on_off(i.github_webhook_enabled)),
        ("Docker", docker),
    ];
    pairs.extend(resource_pairs(&info));
    outln!("{}", output::render_kv(&pairs, output::stdout_color()).trim_end())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn info(extra: Value) -> Json<ServerInfo> {
        let mut raw = json!({
            "version": "0.1.0", "base_domain": "localhost", "proxy_url": "http://localhost:8080",
            "tls_enabled": false, "dashboard_url": null, "github_webhook_enabled": false, "docker_version": "27.3.1"
        });
        for (k, v) in extra.as_object().unwrap() {
            raw[k] = v.clone();
        }
        Json { data: serde_json::from_value(raw.clone()).unwrap(), raw }
    }

    fn texts(info: &Json<ServerInfo>) -> Vec<String> {
        let pairs = resource_pairs(info);
        output::render_kv(&pairs, false).lines().map(str::to_string).collect()
    }

    #[test]
    fn resource_lines_only_when_reported() {
        assert!(texts(&info(json!({}))).is_empty(), "an older server says nothing (not 'unlimited')");
        let i = info(json!({
            "default_memory_limit_mb": 512, "default_cpu_limit": 1.0,
            "docker_cpus": 8, "docker_memory_bytes": 16_u64 * 1024 * 1024 * 1024
        }));
        assert_eq!(
            texts(&i),
            vec!["Default limits:  512 MiB memory, 1 CPU per container", "Docker host:     8 CPUs, 16.0 GiB memory"]
        );
        let i = info(json!({ "default_memory_limit_mb": 0, "default_cpu_limit": 0.5, "docker_cpus": 1 }));
        assert_eq!(
            texts(&i),
            vec!["Default limits:  unlimited memory, 0.5 CPU per container", "Docker host:     1 CPU"]
        );
        let i = info(json!({ "default_memory_limit_mb": 2048, "default_cpu_limit": 0.0 }));
        assert_eq!(texts(&i), vec!["Default limits:  2 GiB memory, unlimited CPU per container"]);
        let i = info(json!({ "default_memory_limit_mb": 0, "default_cpu_limit": 0.0 }));
        assert_eq!(texts(&i), vec!["Default limits:  none (containers are unlimited)"]);
    }
}
