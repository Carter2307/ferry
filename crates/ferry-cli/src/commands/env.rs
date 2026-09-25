//! `ferry env …` (service variables) and `ferry env-group …`.

use anyhow::Result;
use ferry_core::EnvVar;
use ferry_core::dto::{CreateEnvGroup, EnvGroupView, LinkEnvGroup, PatchEnv, ServiceView};

use super::{Ctx, confirm, print_json};
use crate::output::{self, Cell, Table, errln, outln};

fn print_vars(vars: &[EnvVar]) -> Result<()> {
    for v in vars {
        outln!("{}={}", v.key, v.value)?;
    }
    Ok(())
}

fn restart_query(restart: bool) -> Vec<(&'static str, String)> {
    vec![("restart", restart.to_string())]
}

pub async fn list(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<Vec<EnvVar>>(&["services", name, "env"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        errln!("No environment variables set on '{name}'. Add some with: ferry env set {name} KEY=VALUE");
    }
    print_vars(&resp.data)
}

async fn patch(ctx: &Ctx, name: &str, body: PatchEnv, restart: bool) -> Result<()> {
    let resp = ctx.client.patch::<_, Vec<EnvVar>>(&["services", name, "env"], &restart_query(restart), &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    print_vars(&resp.data)?;
    if restart {
        errln!("Saved. '{name}' restarts with the new environment if it is live.");
    } else {
        errln!("Saved without restarting. Apply with: ferry restart {name}");
    }
    Ok(())
}

pub async fn set(ctx: &Ctx, name: &str, vars: Vec<EnvVar>, restart: bool) -> Result<()> {
    patch(ctx, name, PatchEnv { set: vars, unset: Vec::new() }, restart).await
}

pub async fn unset(ctx: &Ctx, name: &str, keys: Vec<String>, restart: bool) -> Result<()> {
    patch(ctx, name, PatchEnv { set: Vec::new(), unset: keys }, restart).await
}

// Env groups ---------------------------------------------------------------

fn print_group(ctx: &Ctx, resp: &crate::client::Json<EnvGroupView>) -> Result<()> {
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
    group_patch(ctx, name, PatchEnv { set: vars, unset: Vec::new() }, restart).await
}

pub async fn group_unset(ctx: &Ctx, name: &str, keys: Vec<String>, restart: bool) -> Result<()> {
    group_patch(ctx, name, PatchEnv { set: Vec::new(), unset: keys }, restart).await
}

pub async fn group_remove(ctx: &Ctx, name: &str, yes: bool) -> Result<()> {
    confirm(&format!("delete env group '{name}'"), yes).await?;
    ctx.client.delete_no_content(&["env-groups", name]).await?;
    if ctx.json {
        return print_json(&serde_json::json!({ "deleted": name }));
    }
    outln!("Deleted env group '{name}'")?;
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
