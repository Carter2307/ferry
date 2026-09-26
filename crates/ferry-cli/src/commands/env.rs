//! `ferry env …` (service variables) and `ferry env-group …`.

use std::borrow::Cow;
use std::fmt::Write as _;

use anyhow::Result;
use ferry_core::dto::{CreateEnvGroup, DatastoreView, EnvGroupView, LinkEnvGroup, PatchEnv, ServiceView};
use ferry_core::{Deploy, DeployTrigger, EnvVar, env};

use super::{Ctx, confirm, print_json, queued_deploy};
use crate::client::Json;
use crate::envref;
use crate::output::{self, Cell, Table, errln, outln};

/// A value as printed in `KEY=VALUE` listings: unchanged when it is a
/// plain single line; otherwise double-quoted with backslash escapes
/// (`\n`, `\r`, `\t`, `\"`, `\\`, `\u{…}`), so every variable stays on one
/// line and control characters never reach the terminal raw.
pub fn display_value(value: &str) -> Cow<'_, str> {
    if !value.starts_with('"') && !value.chars().any(char::is_control) {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    Cow::Owned(out)
}

fn print_vars(vars: &[EnvVar]) -> Result<()> {
    for v in vars {
        outln!("{}={}", v.key, display_value(&v.value))?;
    }
    Ok(())
}

fn restart_query(restart: bool) -> Vec<(&'static str, String)> {
    vec![("restart", restart.to_string())]
}

pub async fn list(ctx: &Ctx, name: &str, effective: bool) -> Result<()> {
    if effective {
        return list_effective(ctx, name).await;
    }
    let resp = ctx.client.get::<Vec<EnvVar>>(&["services", name, "env"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    // Linked env groups contribute variables too (best effort: only a hint).
    let groups = match ctx.client.get::<ServiceView>(&["services", name], &[]).await {
        Ok(v) => v.data.env_groups,
        Err(_) => Vec::new(),
    };
    print_vars(&resp.data)?;
    let groups = groups.join(", ");
    match (resp.data.is_empty(), groups.is_empty()) {
        (true, true) => {
            errln!("No environment variables set on '{name}'. Add some with: ferry env set {name} KEY=VALUE");
        }
        (true, false) => errln!(
            "No variables set on '{name}' itself; it gets those of its linked env group(s) {groups}. \
             Show them with: ferry env {name} --effective"
        ),
        (false, false) => errln!(
            "Also inherited from linked env group(s) {groups} (the variables above take precedence). \
             Show the merged set with: ferry env {name} --effective"
        ),
        (false, true) => {}
    }
    Ok(())
}

/// The environment a new deploy gets: linked env groups in link order, then
/// the service's own variables (later layers win). References are resolved
/// and Ferry's variables (`PORT`, `FERRY_*`) added when the service starts.
async fn list_effective(ctx: &Ctx, name: &str) -> Result<()> {
    let view = ctx.client.get::<ServiceView>(&["services", name], &[]).await?.data;
    let own = ctx.client.get::<Vec<EnvVar>>(&["services", &view.service.id, "env"], &[]).await?.data;
    let mut layers = Vec::new();
    for g in &view.env_groups {
        layers.push(ctx.client.get::<EnvGroupView>(&["env-groups", g], &[]).await?.data.vars);
    }
    layers.push(own);
    let merged = env::merge(&layers);
    if ctx.json {
        return print_json(&serde_json::to_value(&merged)?);
    }
    print_vars(&merged)?;
    let sources = if view.env_groups.is_empty() {
        "the service's own variables".to_string()
    } else {
        format!("env group(s) {}, then '{}''s own variables (which win)", view.env_groups.join(", "), view.service.name)
    };
    errln!(
        "{}",
        output::err(
            output::Color::Dim,
            &format!("Merged from {sources}. ${{{{…}}}} references are resolved, and PORT/FERRY_* added, at start.")
        )
    );
    Ok(())
}

/// Warn about variables whose references don't resolve against the
/// server's current datastores and services (best effort).
async fn warn_unresolved(ctx: &Ctx, vars: &[EnvVar]) {
    if !envref::any_reference(vars) {
        return;
    }
    let (Ok(datastores), Ok(services)) = (
        ctx.client.get::<Vec<DatastoreView>>(&["datastores"], &[]).await,
        ctx.client.get::<Vec<ServiceView>>(&["services"], &[]).await,
    ) else {
        return;
    };
    for (key, why) in envref::unresolved(vars, &datastores.data, &services.data) {
        errln!("{} {key}: {why}; deploys fail until it resolves", output::err(output::Color::Yellow, "warning:"));
    }
}

/// The `env_change` deploy the server queued for `before`'s service after an
/// env change: the newest such deploy that is newer than `before`'s latest.
async fn env_change_deploy(ctx: &Ctx, before: &ServiceView) -> Option<Json<Deploy>> {
    let resp =
        ctx.client.get::<Vec<Deploy>>(&["services", &before.service.id, "deploys"], &[("limit", "10".into())]).await;
    let resp = resp.ok()?;
    let previous = before.latest_deploy.as_ref().map(|d| d.id.as_str());
    let i = resp
        .data
        .iter()
        .take_while(|d| Some(d.id.as_str()) != previous)
        .position(|d| d.trigger == DeployTrigger::EnvChange)?;
    Some(Json { data: resp.data[i].clone(), raw: resp.raw.get(i).cloned().unwrap_or_default() })
}

async fn patch(ctx: &Ctx, name: &str, body: PatchEnv, restart: bool, follow: bool) -> Result<()> {
    // Remember the latest deploy, to recognize the restart the server queues.
    let before = if restart { ctx.client.get::<ServiceView>(&["services", name], &[]).await.ok() } else { None };
    let resp = ctx.client.patch::<_, Vec<EnvVar>>(&["services", name, "env"], &restart_query(restart), &body).await?;
    let before = before.map(|b| b.data);
    let shown = before.as_ref().map_or(name, |b| b.service.name.as_str());
    let restarts = before.as_ref().is_some_and(|b| b.service.live_deploy_id.is_some() && !b.service.suspended);
    let deploy = match &before {
        Some(b) if restarts => env_change_deploy(ctx, b).await,
        _ => None,
    };

    if ctx.json {
        return match &deploy {
            Some(d) if follow => super::follow_deploy(ctx, &d.data.id).await,
            _ => print_json(&resp.raw),
        };
    }
    print_vars(&resp.data)?;
    if !restart {
        errln!("Saved without restarting. Apply with: ferry restart {shown}");
        return Ok(());
    }
    match (&before, deploy) {
        (_, Some(d)) => {
            errln!("Saved. Restarting '{shown}' with the new environment.");
            queued_deploy(ctx, &d, follow).await
        }
        (Some(_), None) if !restarts => {
            errln!("Saved. '{shown}' isn't live, so nothing restarts: its next deploy uses the new environment.");
            Ok(())
        }
        (Some(_), None) => {
            errln!("Saved. '{shown}' restarts with the new environment (see: ferry deploys {shown}).");
            Ok(())
        }
        (None, None) => {
            errln!("Saved. '{shown}' restarts with the new environment if it is live.");
            Ok(())
        }
    }
}

pub async fn set(ctx: &Ctx, name: &str, vars: Vec<EnvVar>, restart: bool, follow: bool) -> Result<()> {
    warn_unresolved(ctx, &vars).await;
    patch(ctx, name, PatchEnv { set: vars, unset: Vec::new() }, restart, follow).await
}

pub async fn unset(ctx: &Ctx, name: &str, keys: Vec<String>, restart: bool, follow: bool) -> Result<()> {
    patch(ctx, name, PatchEnv { set: Vec::new(), unset: keys }, restart, follow).await
}

// Env groups ---------------------------------------------------------------

fn print_group(ctx: &Ctx, resp: &Json<EnvGroupView>) -> Result<()> {
    if ctx.json {
        return print_json(&resp.raw);
    }
    let g = &resp.data;
    outln!("{} ({})", output::out(output::Color::Bold, &g.group.name), g.group.id)?;
    let services = if g.services.is_empty() { "none".to_string() } else { g.services.join(", ") };
    outln!("Linked services: {services}")?;
    if g.vars.is_empty() {
        outln!("No variables.")?;
    } else {
        outln!()?;
        print_vars(&g.vars)?;
    }
    Ok(())
}

pub async fn group_create(ctx: &Ctx, name: &str, vars: Vec<EnvVar>) -> Result<()> {
    warn_unresolved(ctx, &vars).await;
    let body = CreateEnvGroup { name: name.to_string(), vars };
    let resp = ctx.client.post::<_, EnvGroupView>(&["env-groups"], &[], &body).await?;
    if !ctx.json {
        errln!(
            "Created env group '{}'. Link it with: ferry env-group link SERVICE {}",
            resp.data.group.name,
            resp.data.group.name
        );
    }
    print_group(ctx, &resp)
}

pub async fn group_list(ctx: &Ctx) -> Result<()> {
    let resp = ctx.client.get::<Vec<EnvGroupView>>(&["env-groups"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No env groups yet. Create one with: ferry env-group create NAME KEY=VALUE…")?;
        return Ok(());
    }
    let mut t = Table::new(&["NAME", "ID", "VARS", "SERVICES", "UPDATED"]);
    for g in &resp.data {
        t.row(vec![
            Cell::new(g.group.name.as_str()),
            Cell::new(g.group.id.as_str()),
            Cell::new(g.vars.len().to_string()),
            Cell::new(if g.services.is_empty() { "-".to_string() } else { g.services.join(", ") }),
            Cell::new(output::ago(Some(g.group.updated_at))),
        ]);
    }
    outln!("{}", t.render(output::stdout_color()).trim_end())?;
    Ok(())
}

pub async fn group_show(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<EnvGroupView>(&["env-groups", name], &[]).await?;
    print_group(ctx, &resp)
}

async fn group_patch(ctx: &Ctx, name: &str, body: PatchEnv, restart: bool) -> Result<()> {
    let resp =
        ctx.client.patch::<_, EnvGroupView>(&["env-groups", name, "env"], &restart_query(restart), &body).await?;
    print_group(ctx, &resp)?;
    if !ctx.json && !resp.data.services.is_empty() {
        if restart {
            errln!("Saved. Linked live services restart with the new environment.");
        } else {
            errln!("Saved without restarting linked services.");
        }
    }
    Ok(())
}

pub async fn group_set(ctx: &Ctx, name: &str, vars: Vec<EnvVar>, restart: bool) -> Result<()> {
    warn_unresolved(ctx, &vars).await;
    group_patch(ctx, name, PatchEnv { set: vars, unset: Vec::new() }, restart).await
}

pub async fn group_unset(ctx: &Ctx, name: &str, keys: Vec<String>, restart: bool) -> Result<()> {
    group_patch(ctx, name, PatchEnv { set: Vec::new(), unset: keys }, restart).await
}

pub async fn group_remove(ctx: &Ctx, name: &str, yes: bool) -> Result<()> {
    // Resolve an id to the group's real name for the prompt and messages.
    let group = ctx.client.get::<EnvGroupView>(&["env-groups", name], &[]).await?.data.group;
    confirm(&format!("delete env group '{}'", group.name), yes).await?;
    ctx.client.delete_no_content(&["env-groups", &group.id]).await?;
    if ctx.json {
        return print_json(&serde_json::json!({ "deleted": group.name, "id": group.id }));
    }
    outln!("Deleted env group '{}'", group.name)?;
    Ok(())
}

pub async fn group_link(ctx: &Ctx, service: &str, group: &str) -> Result<()> {
    let body = LinkEnvGroup { group: group.to_string() };
    let resp = ctx.client.post::<_, ServiceView>(&["services", service, "env-groups"], &[], &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let v = &resp.data;
    outln!("Linked '{group}' to '{}' (groups: {})", v.service.name, v.env_groups.join(", "))?;
    errln!("Restart to apply: ferry restart {}", v.service.name);
    Ok(())
}

pub async fn group_unlink(ctx: &Ctx, service: &str, group: &str) -> Result<()> {
    let resp = ctx.client.delete::<ServiceView>(&["services", service, "env-groups", group]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let v = &resp.data;
    let groups = if v.env_groups.is_empty() { "none".to_string() } else { v.env_groups.join(", ") };
    outln!("Unlinked '{group}' from '{}' (groups: {groups})", v.service.name)?;
    errln!("Restart to apply: ferry restart {}", v.service.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_that_would_break_the_listing_are_quoted() {
        assert_eq!(display_value("plain"), "plain");
        assert_eq!(display_value("x=y with spaces"), "x=y with spaces");
        assert_eq!(display_value(""), "");
        assert_eq!(display_value("it's \"fine\" inside"), "it's \"fine\" inside");
        assert!(matches!(display_value("plain"), Cow::Borrowed(_)));
        assert_eq!(display_value("line1\nline2"), r#""line1\nline2""#);
        assert_eq!(display_value("a\r\n\tb"), r#""a\r\n\tb""#);
        assert_eq!(display_value("\"quoted\""), r#""\"quoted\"""#);
        assert_eq!(display_value("C:\\dir\n"), r#""C:\\dir\n""#);
        assert_eq!(display_value("bell\u{7}esc\u{1b}[31m"), r#""bell\u{7}esc\u{1b}[31m""#);
        assert!(!display_value("a\nb").contains('\n'));
    }
}
