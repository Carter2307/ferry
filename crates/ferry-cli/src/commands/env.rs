//! `ferry env …` (service variables) and `ferry env-group …`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::Result;
use ferry_core::dto::{CreateEnvGroup, DatastoreView, EnvGroupView, LinkEnvGroup, PatchEnv, ServiceView};
use ferry_core::{Deploy, DeployStatus, EnvVar, env};

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

/// Keys of `unset` that aren't set in `vars` (removing them does nothing).
fn missing_keys<'a>(vars: &[EnvVar], unset: &'a [String]) -> Vec<&'a str> {
    unset.iter().filter(|k| !vars.iter().any(|v| &v.key == *k)).map(String::as_str).collect()
}

/// Whether applying `patch` to `vars` changes anything.
fn changes(vars: &[EnvVar], patch: &PatchEnv) -> bool {
    let current = |key: &str| vars.iter().find(|v| v.key == key).map(|v| v.value.as_str());
    patch.set.iter().any(|v| current(&v.key) != Some(v.value.as_str()))
        || patch.unset.iter().any(|k| current(k).is_some())
}

/// Same variables with the same values (order aside).
fn same_vars(a: &[EnvVar], b: &[EnvVar]) -> bool {
    fn map(vars: &[EnvVar]) -> BTreeMap<&str, &str> {
        vars.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect()
    }
    map(a) == map(b)
}

/// What an env change did to the service's deploys, from its latest deploy
/// before and after the change.
#[derive(Debug, Clone, PartialEq)]
enum Restart {
    /// A new deploy: the restart the server queued.
    Queued(Deploy),
    /// A deploy that was already queued: it starts with the new environment
    /// (the server folds the restart into it).
    AlreadyQueued(Deploy),
    /// Nothing restarts.
    Nothing,
}

impl Restart {
    fn of(before: Option<&Deploy>, after: Option<&Deploy>) -> Self {
        match after {
            Some(d) if before.is_none_or(|b| b.id != d.id) => Restart::Queued(d.clone()),
            Some(d) if d.status == DeployStatus::Queued => Restart::AlreadyQueued(d.clone()),
            _ => Restart::Nothing,
        }
    }

    fn deploy(&self) -> Option<&Deploy> {
        match self {
            Restart::Queued(d) | Restart::AlreadyQueued(d) => Some(d),
            Restart::Nothing => None,
        }
    }
}

/// Why nothing restarted after an env change that asked for a restart.
fn no_restart_reason(v: &ServiceView) -> String {
    let n = &v.service.name;
    if v.service.suspended {
        format!(
            "Saved. '{n}' is suspended, so nothing restarts: after resuming it, apply the new environment with: ferry restart {n}"
        )
    } else if v.service.live_deploy_id.is_none() {
        format!("Saved. '{n}' isn't live, so nothing restarts: its next deploy uses the new environment.")
    } else {
        format!("Saved, but no restart was queued. Apply the new environment with: ferry restart {n}")
    }
}

async fn patch(ctx: &Ctx, name: &str, body: PatchEnv, restart: bool, follow: bool) -> Result<()> {
    // The service and its variables before the change: to warn about keys
    // that aren't set, to skip a restart that would change nothing, and to
    // recognize the deploy the server queues for the restart.
    let before = ctx.client.get::<ServiceView>(&["services", name], &[]).await?.data;
    let (id, shown) = (before.service.id.as_str(), before.service.name.as_str());
    let old = ctx.client.get::<Vec<EnvVar>>(&["services", id, "env"], &[]).await?.data;
    let missing = missing_keys(&old, &body.unset);
    if !missing.is_empty() {
        let groups = if before.env_groups.is_empty() {
            String::new()
        } else {
            format!(
                " (variables of the linked env group(s) {} are removed with: ferry env-group unset GROUP KEY)",
                before.env_groups.join(", ")
            )
        };
        errln!(
            "{} not set on '{shown}', nothing to remove: {}{groups}",
            output::err(output::Color::Yellow, "warning:"),
            missing.join(", ")
        );
    }
    let restart = restart && changes(&old, &body);
    let resp = ctx.client.patch::<_, Vec<EnvVar>>(&["services", id, "env"], &restart_query(restart), &body).await?;
    let changed = !same_vars(&old, &resp.data);
    // After: a latest deploy other than before's is the queued restart.
    let after =
        if restart { ctx.client.get::<ServiceView>(&["services", id], &[]).await.ok().map(|r| r.data) } else { None };
    let outcome = after.as_ref().map(|a| Restart::of(before.latest_deploy.as_ref(), a.latest_deploy.as_ref()));
    let pending = outcome.as_ref().and_then(Restart::deploy);

    if ctx.json {
        return match pending {
            Some(d) if follow => super::follow_deploy(ctx, &d.id).await,
            _ => print_json(&resp.raw),
        };
    }
    print_vars(&resp.data)?;
    if !changed && pending.is_none() {
        errln!("No changes.");
        return Ok(());
    }
    if !restart {
        let in_flight = before.latest_deploy.as_ref().is_some_and(|d| d.status.is_active());
        if before.service.live_deploy_id.is_some() || in_flight {
            errln!("Saved without restarting. Apply with: ferry restart {shown}");
        } else {
            errln!("Saved without restarting: the next deploy of '{shown}' uses the new environment.");
        }
        return Ok(());
    }
    let (Some(outcome), Some(after)) = (outcome, after) else {
        errln!("Saved. '{shown}' restarts with the new environment if it is live (see: ferry deploys {shown}).");
        return Ok(());
    };
    let deploy = match outcome {
        Restart::Queued(d) => {
            if after.service.live_deploy_id.is_some() {
                errln!("Saved. Restarting '{shown}' with the new environment.");
            } else {
                errln!("Saved. '{shown}' restarts with the new environment once its current deploy is live.");
            }
            d
        }
        Restart::AlreadyQueued(d) => {
            errln!("Saved. Deploy {} of '{shown}' is queued: it starts with the new environment.", d.id);
            d
        }
        Restart::Nothing => {
            errln!("{}", no_restart_reason(&after));
            return Ok(());
        }
    };
    let raw = serde_json::to_value(&deploy)?;
    queued_deploy(ctx, &Json { data: deploy, raw }, follow).await
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

pub async fn group_remove(ctx: &Ctx, name: &str, yes: bool, force: bool, restart: bool) -> Result<()> {
    // Resolve an id to the group's real name for the prompt and messages.
    let group = ctx.client.get::<EnvGroupView>(&["env-groups", name], &[]).await?.data.group;
    confirm(&format!("delete env group '{}'", group.name), yes).await?;
    ctx.client
        .delete_no_content(&["env-groups", &group.id], &super::force_query(force, restart))
        .await
        .map_err(|e| super::with_force_hint(e, force))?;
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
    use ferry_core::{DeploySource, DeployTrigger};

    fn vars(pairs: &[(&str, &str)]) -> Vec<EnvVar> {
        pairs.iter().map(|(k, v)| EnvVar::new(*k, *v)).collect()
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    fn deploy(id: &str, status: DeployStatus) -> Deploy {
        let mut d = Deploy::new("srv-web", DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        d.id = id.into();
        d.status = status;
        d
    }

    #[test]
    fn what_a_patch_changes() {
        let current = vars(&[("A", "1"), ("B", "")]);
        let set = |p: &[(&str, &str)]| PatchEnv { set: vars(p), unset: Vec::new() };
        let unset = |k: &[&str]| PatchEnv { set: Vec::new(), unset: keys(k) };
        assert!(!changes(&current, &set(&[("A", "1")])), "same value");
        assert!(!changes(&current, &set(&[("B", "")])), "same empty value");
        assert!(changes(&current, &set(&[("A", "2")])));
        assert!(changes(&current, &set(&[("C", "")])), "a new variable, even empty");
        assert!(!changes(&current, &unset(&["NOPE"])));
        assert!(changes(&current, &unset(&["NOPE", "B"])));
        assert_eq!(missing_keys(&current, &keys(&["NOPE", "B", "a"])), vec!["NOPE", "a"], "keys are case-sensitive");
        assert!(missing_keys(&current, &[]).is_empty());
        assert!(same_vars(&current, &vars(&[("B", ""), ("A", "1")])), "order doesn't matter");
        assert!(!same_vars(&current, &vars(&[("A", "1")])));
        assert!(!same_vars(&current, &vars(&[("A", "1"), ("B", "x")])));
    }

    #[test]
    fn restart_is_read_from_the_latest_deploy_before_and_after() {
        let live = deploy("dep-1", DeployStatus::Live);
        let building = deploy("dep-1", DeployStatus::Building);
        let queued = deploy("dep-1", DeployStatus::Queued);
        let restart = deploy("dep-2", DeployStatus::Queued);
        // A new latest deploy is the restart (also behind a first deploy still in progress).
        assert_eq!(Restart::of(Some(&live), Some(&restart)), Restart::Queued(restart.clone()));
        assert_eq!(Restart::of(Some(&building), Some(&restart)), Restart::Queued(restart.clone()));
        assert_eq!(Restart::of(None, Some(&restart)), Restart::Queued(restart.clone()));
        // The restart folded into a deploy that was already queued.
        assert_eq!(Restart::of(Some(&queued), Some(&queued)), Restart::AlreadyQueued(queued.clone()));
        assert_eq!(Restart::of(Some(&live), Some(&live)), Restart::Nothing);
        assert_eq!(Restart::of(Some(&building), Some(&live)), Restart::Nothing, "same deploy, went live");
        assert_eq!(Restart::of(None, None), Restart::Nothing);
        assert_eq!(Restart::Queued(restart.clone()).deploy().map(|d| d.id.as_str()), Some("dep-2"));
        assert_eq!(Restart::Nothing.deploy(), None);
    }

    #[test]
    fn why_nothing_restarted() {
        let mut service = ferry_core::Service::new("web", ferry_core::ServiceType::WebService);
        let view = |service: &ferry_core::Service| ServiceView {
            service: service.clone(),
            state: ferry_core::ServiceState::Live,
            url: None,
            hosts: vec![],
            internal_host: "web".into(),
            internal_port: None,
            env_groups: vec![],
            latest_deploy: None,
            deploy_hook_path: String::new(),
        };
        assert!(no_restart_reason(&view(&service)).contains("isn't live, so nothing restarts"));
        service.live_deploy_id = Some("dep-1".into());
        assert!(no_restart_reason(&view(&service)).contains("no restart was queued"));
        service.suspended = true;
        let reason = no_restart_reason(&view(&service));
        assert!(reason.contains("is suspended") && reason.contains("ferry restart web"), "{reason}");
    }

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
