//! Service commands: list, create, show, update, delete, suspend/resume,
//! scale, status, open, domains.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use ferry_core::dto::{
    CreateService, DomainRequest, InstanceStatus, RuntimeStatus, ScaleRequest, ServiceView, UpdateService,
};
use ferry_core::{Deploy, Service, ServiceType, SourceKind, resources, validate};

use super::{Ctx, DefaultLimits, confirm, follow_deploy, limit_pairs, or_dash, print_json};
use crate::cli::{CreateArgs, SettingsArgs, UpdateArgs};
use crate::output::{self, Cell, Color, Table, errln, outln};
use crate::repo;

fn opt_vec<T>(v: Vec<T>) -> Option<Vec<T>> {
    if v.is_empty() { None } else { Some(v) }
}

/// `--memory` for a new resource: `default` / `0` means "not set" (the
/// server default), so it is left out.
pub(super) fn explicit_memory(memory: Option<u32>) -> Option<u32> {
    memory.filter(|m| *m > 0)
}

/// `--cpu` for a new resource (see [`explicit_memory`]).
pub(super) fn explicit_cpus(cpus: Option<f64>) -> Option<f64> {
    cpus.filter(|c| *c > 0.0)
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
        memory_limit_mb: explicit_memory(s.memory),
        cpu_limit: explicit_cpus(s.cpu),
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

/// An `UpdateService` changing exactly the given settings. `--memory` /
/// `--cpu default` are sent as 0, which clears the limit (server default).
pub(super) fn settings_update(s: &SettingsArgs) -> UpdateService {
    UpdateService {
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
        memory_limit_mb: s.memory,
        cpu_limit: s.cpu,
        custom_domains: opt_vec(s.domains.clone()),
        ..UpdateService::default()
    }
}

/// Human names of the given settings, e.g. `["start command", "port"]`.
pub(super) fn settings_labels(s: &SettingsArgs) -> Vec<&'static str> {
    [
        (s.runtime.is_some(), "runtime"),
        (s.root_dir.is_some(), "root dir"),
        (s.dockerfile.is_some(), "Dockerfile"),
        (s.build_cmd.is_some(), "build command"),
        (s.start_cmd.is_some(), "start command"),
        (s.publish_dir.is_some(), "publish dir"),
        (s.port.is_some(), "port"),
        (s.health.is_some(), "health check"),
        (s.instances.is_some(), "instances"),
        (s.schedule.is_some(), "schedule"),
        (s.disk.is_some(), "disk"),
        (!s.domains.is_empty(), "custom domains"),
        (s.memory.is_some(), "memory limit"),
        (s.cpu.is_some(), "CPU limit"),
    ]
    .into_iter()
    .filter_map(|(given, label)| given.then_some(label))
    .collect()
}

/// Map `ferry update` flags to the request body.
pub fn update_request(a: &UpdateArgs) -> UpdateService {
    UpdateService {
        repo_url: a.repo.clone(),
        branch: a.branch.clone(),
        image: a.image.clone(),
        auto_deploy: if a.auto_deploy {
            Some(true)
        } else if a.no_auto_deploy {
            Some(false)
        } else {
            None
        },
        ..settings_update(&a.settings)
    }
}

/// `--repo` as sent to the server: a relative path of a local directory
/// becomes absolute (the server would resolve it against its own working
/// directory); other relative paths are refused.
pub(super) fn repo_arg(repo: Option<String>) -> Result<Option<String>> {
    let Some(raw) = repo else { return Ok(None) };
    let cwd = std::env::current_dir().context("reading the current directory")?;
    if let Some(abs) = repo::absolutize(&raw, &cwd) {
        if abs != raw {
            errln!("note: using the local repository {abs}");
        }
        return Ok(Some(abs));
    }
    if repo::is_relative_path(&raw) {
        bail!(
            "--repo '{}': no such directory here; give a git URL or the absolute path of a repository on the server",
            raw.trim()
        );
    }
    Ok(Some(raw))
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

pub async fn create(ctx: &Ctx, mut a: CreateArgs) -> Result<()> {
    a.repo = repo_arg(a.repo.take())?;
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
    let defaults = DefaultLimits::fetch_if_unset(ctx, s.memory_limit_mb, s.cpu_limit).await;
    pairs.extend(limit_pairs(s.memory_limit_mb, s.cpu_limit, defaults));
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
    pairs.push(("Deploy hook", Cell::new(deploy_hook_url(ctx, v))));
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

/// Settings that only take effect with the next deploy. The start command
/// of a cron job is the exception: when it changes, the server restarts the
/// live cron job, so the new command applies from its next run.
fn needs_redeploy(a: &UpdateArgs, service_type: ServiceType) -> bool {
    let s = &a.settings;
    a.repo.is_some()
        || a.branch.is_some()
        || a.image.is_some()
        || s.runtime.is_some()
        || s.root_dir.is_some()
        || s.dockerfile.is_some()
        || s.build_cmd.is_some()
        || (s.start_cmd.is_some() && service_type != ServiceType::CronJob)
        || s.publish_dir.is_some()
        || s.port.is_some()
        || s.health.is_some()
        || s.disk.is_some()
}

/// What to tell the user after `ferry update`: when the changes apply.
/// Resource limits are captured when a deploy or restart starts, so a
/// restart (which reuses the live image) is enough for them.
fn update_hints(a: &UpdateArgs, svc: &Service) -> Vec<String> {
    let mut hints = Vec::new();
    if svc.service_type == ServiceType::CronJob && a.settings.start_cmd.is_some() {
        hints.push(format!(
            "The new start command applies from the next run (scheduled, or now with: ferry run {})",
            svc.name
        ));
    }
    let limits = a.settings.memory.is_some() || a.settings.cpu.is_some();
    let name = &svc.name;
    if needs_redeploy(a, svc.service_type) {
        let what =
            if limits { "Build/deploy settings and resource limits apply" } else { "Build/deploy settings apply" };
        hints.push(format!("{what} to the next deploy: ferry deploy {name}"));
    } else if limits {
        hints.push(if svc.live_deploy_id.is_none() {
            format!("Resource limits apply to the next deploy: ferry deploy {name}")
        } else if svc.suspended {
            format!("Resource limits apply to the next deploy or restart (after: ferry resume {name})")
        } else {
            format!("Resource limits apply to the next deploy or restart: ferry restart {name}")
        });
    }
    hints
}

pub async fn update(ctx: &Ctx, mut a: UpdateArgs) -> Result<()> {
    if a.repo.is_none()
        && a.branch.is_none()
        && a.image.is_none()
        && a.settings.is_empty()
        && !a.auto_deploy
        && !a.no_auto_deploy
    {
        bail!("nothing to update: pass at least one setting (see 'ferry update --help')");
    }
    // "" clears the repo; anything else may be a local path to absolutize.
    if a.repo.as_deref().is_some_and(|r| !r.trim().is_empty()) {
        a.repo = repo_arg(a.repo.take())?;
    }
    let body = update_request(&a);
    let resp = ctx.client.patch::<_, ServiceView>(&["services", &a.name], &[], &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    outln!("Updated '{}'", resp.data.service.name)?;
    for hint in update_hints(&a, &resp.data.service) {
        outln!("{hint}")?;
    }
    Ok(())
}

pub async fn delete(ctx: &Ctx, name: &str, yes: bool, force: bool) -> Result<()> {
    // Resolve an id to the service's real name for the prompt and messages.
    let service = ctx.client.get::<ServiceView>(&["services", name], &[]).await?.data.service;
    confirm(&format!("delete service '{}' with all its deploys", service.name), yes).await?;
    ctx.client
        .delete_no_content(&["services", &service.id], &super::force_query(force, false))
        .await
        .map_err(|e| super::with_force_hint(e, force))?;
    if ctx.json {
        return print_json(&serde_json::json!({ "deleted": service.name, "id": service.id }));
    }
    outln!("Deleted service '{}'", service.name)?;
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
        let cpu = match (i.cpu_percent, i.cpu_limit) {
            (Some(c), Some(l)) if l > 0.0 => format!("{c:.1}% / {}", resources::format_cpus(l)),
            (Some(c), _) => format!("{c:.1}%"),
            _ => "-".to_string(),
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
            Cell::colored(instance_state(i), state_color),
            Cell::new(i.host_port.map(|p| p.to_string()).unwrap_or_else(|| "-".into())),
            Cell::new(cpu),
            Cell::new(memory),
            Cell::new(i.restart_count.map(|r| r.to_string()).unwrap_or_else(|| "-".into())),
            Cell::new(started),
        ]);
    }
    outln!("{}", t.render(output::stdout_color()).trim_end())?;
    if let Some(note) = oom_note(name, &st.instances) {
        errln!("{note}");
    }
    Ok(())
}

/// Killed by the kernel for exceeding its memory limit, last time it
/// exited. Only meaningful for a container that isn't running (again).
fn was_oom_killed(i: &InstanceStatus) -> bool {
    i.oom_killed && i.state != "running"
}

/// The STATE cell: Docker's state, plus why an exited / restarting
/// container stopped (`exited (OOM killed, exit code 137)`).
fn instance_state(i: &InstanceStatus) -> String {
    if i.state == "running" {
        return i.state.clone();
    }
    let code = i.exit_code.filter(|c| *c != 0).map(|c| format!("exit code {c}"));
    match (was_oom_killed(i), code) {
        (true, Some(code)) => format!("{} (OOM killed, {code})", i.state),
        (true, None) => format!("{} (OOM killed)", i.state),
        (false, Some(code)) => format!("{} ({code})", i.state),
        (false, None) => i.state.clone(),
    }
}

/// How to fix instances that ran out of memory, if any did.
fn oom_note(service: &str, instances: &[InstanceStatus]) -> Option<String> {
    let killed: Vec<&InstanceStatus> = instances.iter().filter(|i| was_oom_killed(i)).collect();
    let first = killed.first()?;
    let labels: Vec<String> = killed.iter().map(|i| instance_label(&i.name)).collect();
    let who = match labels.as_slice() {
        [one] => format!("instance {one} was"),
        many => format!("instances {} were", many.join(", ")),
    };
    let limit =
        first.memory_limit_bytes.map(|l| format!(" (limit {})", output::memory_limit_bytes(l))).unwrap_or_default();
    Some(format!(
        "note: {who} killed for running out of memory{limit}. Raise the limit with \
         'ferry update {service} --memory <SIZE>', then 'ferry restart {service}'."
    ))
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

// Deploy hook --------------------------------------------------------------

/// The deploy hook URL: the server URL plus the hook path (which carries the
/// secret key). The hook is served by the API, like `/api/v1`.
pub(super) fn deploy_hook_url(ctx: &Ctx, v: &ServiceView) -> String {
    format!("{}{}", ctx.client.server(), v.deploy_hook_path)
}

pub async fn deploy_hook_show(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<ServiceView>(&["services", name], &[]).await?;
    let url = deploy_hook_url(ctx, &resp.data);
    if ctx.json {
        return print_json(&serde_json::json!({ "service": resp.data.service.name, "url": url }));
    }
    outln!("{url}")?;
    Ok(())
}

/// `ferry deploy-hook rotate NAME`: a new secret key; the old URL stops working.
pub async fn deploy_hook_rotate(ctx: &Ctx, name: &str, yes: bool) -> Result<()> {
    // Resolve an id to the service's real name for the prompt and messages.
    let service = ctx.client.get::<ServiceView>(&["services", name], &[]).await?.data.service;
    confirm(&format!("rotate the deploy hook of '{}' (its current URL stops working)", service.name), yes).await?;
    let resp = match ctx.client.post_empty::<ServiceView>(&["services", &service.id, "deploy-hook", "rotate"]).await {
        Ok(r) => r,
        Err(e) if crate::client::is_missing_route(&e) => {
            return Err(e.context("this Ferry server can't rotate deploy hooks: upgrade ferryd"));
        }
        Err(e) => return Err(e),
    };
    if ctx.json {
        return print_json(&resp.raw);
    }
    errln!("Rotated the deploy hook of '{}': the old URL no longer works. New URL:", resp.data.service.name);
    outln!("{}", deploy_hook_url(ctx, &resp.data))?;
    Ok(())
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

/// A domain as the server stores it: trimmed, lowercase, no trailing dot.
fn normalize_domain(domain: &str) -> String {
    validate::domain(domain).unwrap_or_else(|_| domain.trim().trim_end_matches('.').to_ascii_lowercase())
}

pub async fn domains_add(ctx: &Ctx, name: &str, domain: &str) -> Result<()> {
    let domain = normalize_domain(domain);
    let body = DomainRequest { domain: domain.clone() };
    let resp = ctx.client.post::<_, Vec<String>>(&["services", name, "domains"], &[], &body).await?;
    if !ctx.json {
        errln!("Added {domain}. Point its DNS (A/AAAA or CNAME) at this server.");
    }
    print_domains(ctx, name, &resp)
}

pub async fn domains_rm(ctx: &Ctx, name: &str, domain: &str) -> Result<()> {
    let domain = normalize_domain(domain);
    let resp = ctx.client.delete::<Vec<String>>(&["services", name, "domains", &domain]).await?;
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
        assert!(needs_redeploy(&a, ServiceType::WebService));
        let Command::Update(a) = parse(&["update", "web", "--instances", "3"]) else { panic!() };
        assert!(!needs_redeploy(&a, ServiceType::WebService));
        assert_eq!(update_request(&a).instances, Some(3));
    }

    #[test]
    fn settings_map_to_an_update_and_labels() {
        let Command::Up(a) = parse(&["up", "--start-cmd", "python -m http.server $PORT", "--port", "8000"]) else {
            panic!()
        };
        let body = settings_update(&a.settings);
        assert_eq!(body.start_command.as_deref(), Some("python -m http.server $PORT"));
        assert_eq!(body.port, Some(8000));
        assert_eq!((body.repo_url, body.image, body.auto_deploy, body.suspended), (None, None, None, None));
        assert_eq!(settings_labels(&a.settings), vec!["start command", "port"]);
        assert!(settings_labels(&SettingsArgs::default()).is_empty());
    }

    #[test]
    fn cron_start_command_applies_at_the_next_run() {
        let cron = Service::new("nightly", ServiceType::CronJob);
        let Command::Update(a) = parse(&["update", "nightly", "--start-cmd", "echo updated"]) else { panic!() };
        let hints = update_hints(&a, &cron);
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert!(hints[0].contains("next run") && hints[0].contains("ferry run nightly"), "{hints:?}");
        // Build settings of a cron job still need a deploy (they make the image).
        let Command::Update(a) = parse(&["update", "nightly", "--start-cmd", "x", "--build-cmd", "make"]) else {
            panic!()
        };
        let hints = update_hints(&a, &cron);
        assert_eq!(hints.len(), 2, "{hints:?}");
        assert!(hints[1].contains("ferry deploy nightly"));
        // Other services: the start command is baked into the deploy.
        let web = Service::new("web", ServiceType::WebService);
        let Command::Update(a) = parse(&["update", "web", "--start-cmd", "x"]) else { panic!() };
        assert_eq!(update_hints(&a, &web), vec!["Build/deploy settings apply to the next deploy: ferry deploy web"]);
        let Command::Update(a) = parse(&["update", "nightly", "--schedule", "@daily"]) else { panic!() };
        assert!(update_hints(&a, &cron).is_empty());
    }

    #[test]
    fn limits_map_to_requests() {
        // Create: explicit limits are sent; `default` / 0 are left out.
        let Command::Create(a) = parse(&["create", "api", "--memory", "1.5G", "--cpu", "500m"]) else { panic!() };
        let body = create_request(&a);
        assert_eq!((body.memory_limit_mb, body.cpu_limit), (Some(1536), Some(0.5)));
        let Command::Create(a) = parse(&["create", "api", "--memory", "default", "--cpu", "0"]) else { panic!() };
        let json = serde_json::to_value(create_request(&a)).unwrap();
        assert!(json["memory_limit_mb"].is_null() && json["cpu_limit"].is_null(), "{json}");
        let Command::Create(a) = parse(&["create", "api"]) else { panic!() };
        assert_eq!((create_request(&a).memory_limit_mb, create_request(&a).cpu_limit), (None, None));

        // Update: `default` / 0 are sent as 0 (clear → server default).
        let Command::Update(a) = parse(&["update", "api", "--memory", "512", "--cpu", "2"]) else { panic!() };
        let body = update_request(&a);
        assert_eq!((body.memory_limit_mb, body.cpu_limit), (Some(512), Some(2.0)));
        assert_eq!(settings_labels(&a.settings), vec!["memory limit", "CPU limit"]);
        assert!(!needs_redeploy(&a, ServiceType::WebService), "a restart is enough");
        let Command::Update(a) = parse(&["update", "api", "--memory", "DEFAULT", "--cpu", "default"]) else { panic!() };
        let json = serde_json::to_value(update_request(&a)).unwrap();
        assert_eq!((json["memory_limit_mb"].clone(), json["cpu_limit"].clone()), (0.into(), 0.0.into()));
        let Command::Update(a) = parse(&["update", "api", "--cpu", "0"]) else { panic!() };
        assert_eq!((update_request(&a).memory_limit_mb, update_request(&a).cpu_limit), (None, Some(0.0)));
    }

    #[test]
    fn limit_changes_apply_with_the_next_deploy_or_restart() {
        let mut web = Service::new("web", ServiceType::WebService);
        let Command::Update(a) = parse(&["update", "web", "--memory", "1G"]) else { panic!() };
        // Never deployed: the first deploy uses them.
        assert_eq!(update_hints(&a, &web), vec!["Resource limits apply to the next deploy: ferry deploy web"]);
        web.live_deploy_id = Some("dep-1".into());
        assert_eq!(
            update_hints(&a, &web),
            vec!["Resource limits apply to the next deploy or restart: ferry restart web"]
        );
        web.suspended = true;
        assert!(update_hints(&a, &web)[0].contains("after: ferry resume web"), "{:?}", update_hints(&a, &web));
        web.suspended = false;
        // With build settings: one deploy applies both.
        let Command::Update(a) = parse(&["update", "web", "--cpu", "default", "--build-cmd", "make"]) else { panic!() };
        assert_eq!(
            update_hints(&a, &web),
            vec!["Build/deploy settings and resource limits apply to the next deploy: ferry deploy web"]
        );
    }

    fn instance(state: &str, oom_killed: bool, exit_code: Option<i64>) -> InstanceStatus {
        InstanceStatus {
            container_id: "c".into(),
            name: "/ferry-web-cdefghij-a1b2c3".into(),
            deploy_id: None,
            state: state.into(),
            host_port: None,
            started_at: None,
            restart_count: None,
            cpu_percent: None,
            memory_bytes: None,
            memory_limit_bytes: Some(256 * 1024 * 1024),
            cpu_limit: None,
            oom_killed,
            exit_code,
        }
    }

    #[test]
    fn oom_killed_instances_are_marked() {
        assert_eq!(instance_state(&instance("exited", true, Some(137))), "exited (OOM killed, exit code 137)");
        assert_eq!(instance_state(&instance("restarting", true, None)), "restarting (OOM killed)");
        assert_eq!(instance_state(&instance("exited", false, Some(1))), "exited (exit code 1)");
        assert_eq!(instance_state(&instance("exited", false, Some(0))), "exited");
        // Exit code 137 alone is a SIGKILL, not proof of OOM.
        assert_eq!(instance_state(&instance("exited", false, Some(137))), "exited (exit code 137)");
        // Running again: the flag describes an older exit.
        assert_eq!(instance_state(&instance("running", true, Some(137))), "running");

        assert_eq!(oom_note("web", &[instance("running", true, None), instance("exited", false, Some(1))]), None);
        let note = oom_note("web", &[instance("exited", true, Some(137))]).unwrap();
        assert_eq!(
            note,
            "note: instance a1b2c3 was killed for running out of memory (limit 256 MiB). Raise the limit with \
             'ferry update web --memory <SIZE>', then 'ferry restart web'."
        );
        let mut other = instance("restarting", true, None);
        other.name = "/ferry-web-cdefghij-d4e5f6".into();
        let note = oom_note("web", &[instance("exited", true, None), other]).unwrap();
        assert!(note.starts_with("note: instances a1b2c3, d4e5f6 were killed"), "{note}");
    }

    #[test]
    fn domains_are_normalized_like_the_server() {
        assert_eq!(normalize_domain(" spaced.com "), "spaced.com");
        assert_eq!(normalize_domain("newsvc2.LOCALHOST"), "newsvc2.localhost");
        assert_eq!(normalize_domain("NEWSVC.localhost."), "newsvc.localhost");
        // Invalid ones are still normalized (the server explains what's wrong).
        assert_eq!(normalize_domain(" 1.2.3.4. "), "1.2.3.4");
    }

    #[test]
    fn instance_labels() {
        assert_eq!(instance_label("/ferry-web-1234abcd-a1b2c3"), "a1b2c3");
        assert_eq!(instance_label("abc"), "abc");
    }
}
