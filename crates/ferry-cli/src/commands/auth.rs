//! `ferry login` and `ferry info`.

use anyhow::{Context, Result, anyhow};
use ferry_core::dto::ServerInfo;

use super::{Ctx, print_json};
use crate::client::Client;
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
    if settings.server_source == Source::Default {
        errln!("note: no --server given, using the default {}", config::DEFAULT_SERVER);
    }
    Ok(())
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
    let pairs = vec![
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
    outln!("{}", output::render_kv(&pairs, output::stdout_color()).trim_end())?;
    Ok(())
}
