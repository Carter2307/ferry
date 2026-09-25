//! Service commands: list, create, show, update, delete, suspend/resume,
//! scale, status, open, domains.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use ferry_core::dto::{CreateService, DomainRequest, RuntimeStatus, ScaleRequest, ServiceView, UpdateService};
use ferry_core::{Deploy, ServiceType, SourceKind};

use super::{Ctx, confirm, follow_deploy, or_dash, print_json};
use crate::cli::{CreateArgs, SettingsArgs, UpdateArgs};
use crate::output::{self, Cell, Color, Table, errln, outln};

fn opt_vec<T>(v: Vec<T>) -> Option<Vec<T>> {
    if v.is_empty() { None } else { Some(v) }
}

/// A `CreateService` with the shared settings filled in.
pub(super) fn create_body(name: &str, service_type: Option<ServiceType>, s: &SettingsArgs) -> CreateService {
    CreateService {
        name: name.to_string(),
        service_type,
        runtime: s.runtime,
        root_dir: s.root_dir.clone(),
        dockerfile_path: s.dockerfile.clone(),
        build_command: s.build_cmd.clone(),
        start_command: s.start_cmd.clone(),
        publish_dir: s.publish_dir.clone(),
        port: s.port,
        health_check_path: s.health.clone(),
        schedule: s.schedule.clone(),
        instances: s.instances,
        disk_mount_path: s.disk.clone(),
        custom_domains: opt_vec(s.domains.clone()),
        ..CreateService::default()
    }
}

/// Map `ferry create` flags to the request body.
pub fn create_request(a: &CreateArgs) -> CreateService {
    CreateService {
        repo_url: a.repo.clone(),
        branch: a.branch.clone(),
        image: a.image.clone(),
        auto_deploy: a.no_auto_deploy.then_some(false),
        env: opt_vec(a.env.clone()),
        env_groups: opt_vec(a.env_groups.clone()),
        deploy: a.no_deploy.then_some(false),
        ..create_body(&a.name, a.service_type, &a.settings)
    }
}

/// Map `ferry update` flags to the request body.
pub fn update_request(a: &UpdateArgs) -> UpdateService {
    let s = &a.settings;
    UpdateService {
        repo_url: a.repo.clone(),
        branch: a.branch.clone(),
        image: a.image.clone(),
        runtime: s.runtime,
        root_dir: s.root_dir.clone(),
        dockerfile_path: s.dockerfile.clone(),
        build_command: s.build_cmd.clone(),
        start_command: s.start_cmd.clone(),
        publish_dir: s.publish_dir.clone(),
        port: s.port,
        health_check_path: s.health.clone(),
        schedule: s.schedule.clone(),
        instances: s.instances,
        auto_deploy: if a.auto_deploy {
            Some(true)
        } else if a.no_auto_deploy {
            Some(false)
        } else {
            None
        },
        suspended: None,
        disk_mount_path: s.disk.clone(),
        custom_domains: opt_vec(s.domains.clone()),
    }
}

/// Address shown for a service: public URL, private `host:port`, or `-`.
fn address(v: &ServiceView) -> String {
    if let Some(u) = &v.url {
        return u.clone();
    }
    match v.service.service_type {
        ServiceType::PrivateService => match v.internal_port {
            Some(p) => format!("{}:{p}", v.internal_host),
            None => v.internal_host.clone(),
        },
        ServiceType::CronJob => v.service.schedule.as_deref().map(|s| format!("cron: {s}")).unwrap_or_default(),
        _ => String::new(),
    }
}

fn deploy_summary(d: Option<&Deploy>) -> Cell {
    match d {
        Some(d) => Cell::colored(
            format!("{} · {}", d.status, output::relative_time(d.created_at, Utc::now())),
            output::deploy_status_color(d.status),
        ),
        None => Cell::new("-"),
    }
}

pub async fn list(ctx: &Ctx) -> Result<()> {
    let resp = ctx.client.get::<Vec<ServiceView>>(&["services"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No services yet. Create one with 'ferry create NAME' or deploy a directory with 'ferry up'.")?;
        return Ok(());
    }
    let mut t = Table::new(&["NAME", "TYPE", "STATE", "INSTANCES", "URL", "LAST DEPLOY"]);
    for v in &resp.data {
        let instances =
            if v.service.is_long_running() { v.service.desired_instances().to_string() } else { "-".into() };
        t.row(vec![
            Cell::new(&v.service.name),
            Cell::new(output::type_short(v.service.service_type)),
            Cell::colored(v.state.as_str(), output::service_state_color(v.state)),
            Cell::new(instances),
            Cell::new(address(v)),
            deploy_summary(v.latest_deploy.as_ref()),
        ]);
    }
    outln!("{}", t.render(output::stdout_color()).trim_end())?;
    Ok(())
}

pub async fn create(ctx: &Ctx, a: CreateArgs) -> Result<()> {
    let body = create_request(&a);
    let resp = ctx.client.post::<_, ServiceView>(&["services"], &[], &body).await?;
    let v = &resp.data;
    if ctx.json {
        print_json(&resp.raw)?;
    } else {
        outln!("Created {} '{}' ({})", output::type_long(v.service.service_type), v.service.name, v.service.id)?;
        if let Some(u) = &v.url {
            outln!("URL: {u}")?;
        }
    }
    match &v.latest_deploy {
        Some(d) if a.follow => {
            if !ctx.json {
                errln!("Deploy {} {} ({}), streaming logs…", d.id, d.status, d.trigger);
            }
            follow_deploy(ctx, &d.id).await
        }
        Some(d) => {
            if !ctx.json {
                outln!(
                    "Deploy {} {} ({})",
                    d.id,
                    output::out_opt(output::deploy_status_color(d.status), d.status.as_str()),
                    d.trigger
                )?;
                outln!("Follow it with: ferry logs --deploy {} -f", d.id)?;
            }
            Ok(())
        }
        None => {
            if !ctx.json {
                if v.service.source_kind() == SourceKind::Upload {
                    outln!("Deploy local code with: ferry up {} --dir <DIR>", v.service.name)?;
                } else {
                    outln!("Deploy it with: ferry deploy {}", v.service.name)?;
                }
            }
            Ok(())
        }
    }
}

pub async fn show(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<ServiceView>(&["services", name], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let v = &resp.data;
    let s = &v.service;
    let mut pairs: Vec<(&str, Cell)> = vec![
        ("Name", Cell::new(s.name.as_str())),
        ("ID", Cell::new(s.id.as_str())),
        ("Type", Cell::new(output::type_long(s.service_type))),
        ("State", Cell::colored(v.state.as_str(), output::service_state_color(v.state))),
    ];
    if let Some(u) = &v.url {
        pairs.push(("URL", Cell::new(u.as_str())));
    }
    if v.hosts.len() > 1 {
        pairs.push(("Hosts", Cell::new(v.hosts.join(", "))));
    }
    let internal = match v.internal_port {
        Some(p) => format!("{}:{p}", v.internal_host),
        None => v.internal_host.clone(),
    };
    pairs.push(("Internal address", Cell::new(internal)));
    let source = match s.source_kind() {
        SourceKind::Git => format!("git {} (branch {})", or_dash(s.repo_url.as_deref()), s.branch),
        SourceKind::Image => format!("image {}", or_dash(s.image.as_deref())),
        SourceKind::Upload => "local upload (ferry up)".to_string(),
    };
    pairs.push(("Source", Cell::new(source)));
    pairs.push(("Runtime", Cell::new(s.runtime.as_str())));
    let optional = [
        ("Root dir", &s.root_dir),
        ("Dockerfile", &s.dockerfile_path),
        ("Build command", &s.build_command),
        ("Start command", &s.start_command),
        ("Publish dir", &s.publish_dir),
        ("Health check", &s.health_check_path),
        ("Schedule", &s.schedule),
        ("Disk", &s.disk_mount_path),
    ];
    for (k, val) in optional {
        if let Some(val) = val {
            pairs.push((k, Cell::new(val.as_str())));
        }
    }
    pairs.push(("Port", Cell::new(s.port.map(|p| p.to_string()).unwrap_or_else(|| "auto".to_string()))));
    if s.is_long_running() {
        pairs.push(("Instances", Cell::new(s.instances.to_string())));
    }
    pairs.push(("Auto-deploy", Cell::new(if s.auto_deploy { "yes" } else { "no" })));
    if s.suspended {
        pairs.push(("Suspended", Cell::colored("yes", Some(Color::Yellow))));
    }
    if !s.custom_domains.is_empty() {
        pairs.push(("Custom domains", Cell::new(s.custom_domains.join(", "))));
    }
    if !v.env_groups.is_empty() {
        pairs.push(("Env groups", Cell::new(v.env_groups.join(", "))));
    }
    if let Some(d) = &v.latest_deploy {
        let when = output::relative_time(d.created_at, Utc::now());
        let mut line = format!("{} {} ({}, {when})", d.id, d.status, d.trigger);
        if let Some(e) = &d.error {
            line.push_str(&format!(" — {e}"));
        }
        pairs.push(("Latest deploy", Cell::colored(line, output::deploy_status_color(d.status))));
    }
    if let Some(live) = &s.live_deploy_id {
        pairs.push(("Live deploy", Cell::new(live.as_str())));
    }
    pairs.push(("Deploy hook", Cell::new(format!("{}{}", ctx.client.server(), v.deploy_hook_path))));
    pairs.push(("Created", Cell::new(fmt_time(s.created_at))));
    pairs.push(("Updated", Cell::new(fmt_time(s.updated_at))));
    outln!("{}", output::render_kv(&pairs, output::stdout_color()).trim_end())?;
    Ok(())
}

fn fmt_time(t: DateTime<Utc>) -> String {
    format!(
        "{} ({})",
        t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S"),
        output::relative_time(t, Utc::now())
    )
}

/// Settings that only take effect with the next deploy.
fn needs_redeploy(a: &UpdateArgs) -> bool {
    let s = &a.settings;
    a.repo.is_some()
        || a.branch.is_some()
        || a.image.is_some()
        || s.runtime.is_some()
        || s.root_dir.is_some()
        || s.dockerfile.is_some()
        || s.build_cmd.is_some()
        || s.start_cmd.is_some()
        || s.publish_dir.is_some()
        || s.port.is_some()
        || s.health.is_some()
        || s.disk.is_some()
}

pub async fn update(ctx: &Ctx, a: UpdateArgs) -> Result<()> {
    if a.repo.is_none()
        && a.branch.is_none()
        && a.image.is_none()
        && a.settings.is_empty()
        && !a.auto_deploy
        && !a.no_auto_deploy
    {
        bail!("nothing to update: pass at least one setting (see 'ferry update --help')");
    }
    let body = update_request(&a);
    let resp = ctx.client.patch::<_, ServiceView>(&["services", &a.name], &[], &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    outln!("Updated '{}'", resp.data.service.name)?;
    if needs_redeploy(&a) {
        outln!("Build/deploy settings apply to the next deploy: ferry deploy {}", resp.data.service.name)?;
    }
    Ok(())
}

pub async fn delete(ctx: &Ctx, name: &str, yes: bool) -> Result<()> {
    confirm(&format!("delete service '{name}' with all its deploys"), yes).await?;
    ctx.client.delete_no_content(&["services", name]).await?;
    if ctx.json {
        return print_json(&serde_json::json!({ "deleted": name }));
    }
    outln!("Deleted service '{name}'")?;
    Ok(())
}

pub async fn suspend(ctx: &Ctx, name: &str, suspend: bool) -> Result<()> {
    let action = if suspend { "suspend" } else { "resume" };
    let resp = ctx.client.post_empty::<ServiceView>(&["services", name, action]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let v = &resp.data;
    let verb = if suspend { "Suspended" } else { "Resumed" };
    outln!(
        "{verb} '{}' (state: {})",
        v.service.name,
        output::out_opt(output::service_state_color(v.state), v.state.as_str())
    )?;
    Ok(())
}

pub async fn scale(ctx: &Ctx, name: &str, instances: u32) -> Result<()> {
    let resp =
        ctx.client.post::<_, ServiceView>(&["services", name, "scale"], &[], &ScaleRequest { instances }).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let v = &resp.data;
    outln!("Scaled '{}' to {} instance(s)", v.service.name, v.service.instances)?;
    if v.service.disk_mount_path.is_some() && instances > 1 {
        errln!("note: services with a disk run a single instance");
    }
    Ok(())
}

pub async fn status(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<RuntimeStatus>(&["services", name, "status"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let st = &resp.data;
    let running = st.instances.iter().filter(|i| i.state == "running").count();
    outln!(
        "{name}: {} — {running}/{} instance(s) running",
        output::out_opt(output::service_state_color(st.state), st.state.as_str()),
        st.desired_instances
    )?;
    if st.instances.is_empty() {
        return Ok(());
    }
    let mut t = Table::new(&["INSTANCE", "DEPLOY", "STATE", "PORT", "CPU", "MEMORY", "RESTARTS", "STARTED"]);
    for i in &st.instances {
        let state_color = match i.state.as_str() {
            "running" => Some(Color::Green),
            "restarting" | "created" => Some(Color::Yellow),
            _ => Some(Color::Red),
        };
        let memory = match (i.memory_bytes, i.memory_limit_bytes) {
            (Some(m), Some(l)) if l > 0 => format!("{} / {}", output::human_bytes(m), output::human_bytes(l)),
            (Some(m), _) => output::human_bytes(m),
            _ => "-".to_string(),
        };
        let started = i
            .started_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
            .filter(|t| t.timestamp() > 0)
            .map(|t| output::relative_time(t, Utc::now()))
            .unwrap_or_else(|| "-".to_string());
        t.row(vec![
            Cell::new(instance_label(&i.name)),
            Cell::new(i.deploy_id.as_deref().map(ferry_core::ids::short).unwrap_or("-")),
            Cell::colored(i.state.clone(), state_color),
            Cell::new(i.host_port.map(|p| p.to_string()).unwrap_or_else(|| "-".into())),
            Cell::new(i.cpu_percent.map(|c| format!("{c:.1}%")).unwrap_or_else(|| "-".into())),
            Cell::new(memory),
            Cell::new(i.restart_count.map(|r| r.to_string()).unwrap_or_else(|| "-".into())),
            Cell::new(started),
        ]);
    }
    outln!("{}", t.render(output::stdout_color()).trim_end())?;
    Ok(())
}

/// Instance id as shown in runtime logs: the last 6 characters of the container name.
fn instance_label(container_name: &str) -> String {
    let name = container_name.trim_start_matches('/');
    let n = name.chars().count();
    name.chars().skip(n.saturating_sub(6)).collect()
}

pub async fn open(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<ServiceView>(&["services", name], &[]).await?;
    let v = &resp.data;
    let Some(url) = v.url.clone() else {
        let kind = output::type_long(v.service.service_type);
        if v.service.service_type == ServiceType::PrivateService {
            bail!(
                "service '{}' is a {kind} with no public URL; other services reach it at {} on the private network",
                v.service.name,
                address(v)
            );
        }
        bail!("service '{}' is a {kind} and has no URL", v.service.name);
    };
    if ctx.json {
        print_json(&serde_json::json!({ "url": url }))?;
    } else {
        outln!("{url}")?;
    }
    try_open_browser(&url);
    Ok(())
}

/// Best effort: launch the platform's URL opener, never waiting or failing.
fn try_open_browser(url: &str) {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

// Domains ------------------------------------------------------------------

fn print_domains(ctx: &Ctx, name: &str, resp: &crate::client::Json<Vec<String>>) -> Result<()> {
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No custom domains for '{name}'. Add one with: ferry domains add {name} DOMAIN")?;
    }
    for d in &resp.data {
        outln!("{d}")?;
    }
    Ok(())
}

pub async fn domains_list(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<Vec<String>>(&["services", name, "domains"], &[]).await?;
    print_domains(ctx, name, &resp)
}

pub async fn domains_add(ctx: &Ctx, name: &str, domain: &str) -> Result<()> {
    let body = DomainRequest { domain: domain.to_string() };
    let resp = ctx.client.post::<_, Vec<String>>(&["services", name, "domains"], &[], &body).await?;
    if !ctx.json {
        errln!("Added {domain}. Point its DNS (A/AAAA or CNAME) at this server.");
    }
    print_domains(ctx, name, &resp)
}

pub async fn domains_rm(ctx: &Ctx, name: &str, domain: &str) -> Result<()> {
    let resp = ctx.client.delete::<Vec<String>>(&["services", name, "domains", domain]).await?;
    if !ctx.json {
        errln!("Removed {domain}.");
    }
    print_domains(ctx, name, &resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use ferry_core::{EnvVar, Runtime};

    fn parse(args: &[&str]) -> Command {
        Cli::try_parse_from(std::iter::once("ferry").chain(args.iter().copied())).unwrap().command
    }

    #[test]
    fn create_flags_map_to_request() {
        let Command::Create(a) = parse(&[
            "create",
            "api",
            "--type",
            "worker",
            "--image",
            "busybox:stable",
            "--start-cmd",
            "sleep 1000",
            "--env",
            "A=1",
            "--env-group",
            "g",
            "--no-auto-deploy",
            "--no-deploy",
            "--disk",
            "/data",
        ]) else {
            panic!()
        };
        let body = create_request(&a);
        assert_eq!(body.name, "api");
        assert_eq!(body.service_type, Some(ServiceType::BackgroundWorker));
        assert_eq!(body.image.as_deref(), Some("busybox:stable"));
        assert_eq!(body.start_command.as_deref(), Some("sleep 1000"));
        assert_eq!(body.env, Some(vec![EnvVar::new("A", "1")]));
        assert_eq!(body.env_groups, Some(vec!["g".to_string()]));
        assert_eq!(body.auto_deploy, Some(false));
        assert_eq!(body.deploy, Some(false));
        assert_eq!(body.disk_mount_path.as_deref(), Some("/data"));
        assert_eq!(body.custom_domains, None);
        // Unset flags are omitted so the server applies its defaults.
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["type"], "background_worker");
        assert!(json["repo_url"].is_null());

        let Command::Create(a) = parse(&["create", "web"]) else { panic!() };
        let body = create_request(&a);
        assert_eq!((body.service_type, body.auto_deploy, body.deploy, body.env), (None, None, None, None));
    }

    #[test]
    fn update_flags_map_to_request() {
        let Command::Update(a) =
            parse(&["update", "web", "--runtime", "python", "--port", "0", "--auto-deploy", "--domain", "a.com"])
        else {
            panic!()
        };
        let body = update_request(&a);
        assert_eq!(body.runtime, Some(Runtime::Python));
        assert_eq!(body.port, Some(0));
        assert_eq!(body.auto_deploy, Some(true));
        assert_eq!(body.custom_domains, Some(vec!["a.com".to_string()]));
        assert_eq!(body.instances, None);
        assert!(needs_redeploy(&a));
        let Command::Update(a) = parse(&["update", "web", "--instances", "3"]) else { panic!() };
        assert!(!needs_redeploy(&a));
        assert_eq!(update_request(&a).instances, Some(3));
    }

    #[test]
    fn instance_labels() {
        assert_eq!(instance_label("/ferry-web-1234abcd-a1b2c3"), "a1b2c3");
        assert_eq!(instance_label("abc"), "abc");
    }
}
