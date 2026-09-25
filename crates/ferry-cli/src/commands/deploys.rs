//! Deploy commands: deploy, deploys, cancel, rollback, restart.

use anyhow::Result;
use ferry_core::Deploy;
use ferry_core::dto::{RollbackRequest, TriggerDeploy};

use super::{Ctx, print_json, queued_deploy};
use crate::cli::DeployArgs;
use crate::output::{self, Cell, Table, outln};

pub async fn deploy(ctx: &Ctx, a: DeployArgs) -> Result<()> {
    let body = TriggerDeploy { commit: a.commit.clone(), clear_cache: a.clear_cache };
    let resp = ctx.client.post::<_, Deploy>(&["services", &a.name, "deploys"], &[], &body).await?;
    queued_deploy(ctx, &resp, a.follow).await
}

/// `abc1234 first line of the message` (or `-`).
fn commit_summary(d: &Deploy) -> String {
    let sha: String = d.commit_sha.as_deref().unwrap_or("").chars().take(7).collect();
    let msg = d.commit_message.as_deref().and_then(|m| m.lines().next()).unwrap_or("").trim();
    match (sha.is_empty(), msg.is_empty()) {
        (true, true) => "-".to_string(),
        (false, true) => sha,
        (true, false) => output::truncate(msg, 48),
        (false, false) => format!("{sha} {}", output::truncate(msg, 40)),
    }
}

pub fn deploys_table(deploys: &[Deploy], color: bool) -> String {
    let mut t = Table::new(&["ID", "STATUS", "TRIGGER", "COMMIT", "CREATED", "DURATION"]);
    for d in deploys {
        t.row(vec![
            Cell::new(d.id.as_str()),
            Cell::colored(d.status.as_str(), output::deploy_status_color(d.status)),
            Cell::new(d.trigger.as_str()),
            Cell::new(commit_summary(d)),
            Cell::new(output::ago(Some(d.created_at))),
            Cell::new(output::span(d.started_at, d.finished_at)),
        ]);
    }
    t.render(color)
}

pub async fn list(ctx: &Ctx, name: &str, limit: u32) -> Result<()> {
    let resp = ctx.client.get::<Vec<Deploy>>(&["services", name, "deploys"], &[("limit", limit.to_string())]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No deploys for '{name}' yet.")?;
        return Ok(());
    }
    outln!("{}", deploys_table(&resp.data, output::stdout_color()).trim_end())?;
    for d in resp.data.iter().filter(|d| d.status.is_failed()) {
        if let Some(e) = &d.error {
            outln!("{} {}: {}", output::out(output::Color::Red, "✗"), d.id, output::truncate(e, 200))?;
        }
    }
    Ok(())
}

pub async fn cancel(ctx: &Ctx, deploy_id: &str) -> Result<()> {
    let resp = ctx.client.post_empty::<Deploy>(&["deploys", deploy_id, "cancel"]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let d = &resp.data;
    outln!("Deploy {} {}", d.id, output::out_opt(output::deploy_status_color(d.status), d.status.as_str()))?;
    Ok(())
}

pub async fn rollback(ctx: &Ctx, name: &str, deploy_id: &str, follow: bool) -> Result<()> {
    let body = RollbackRequest { deploy_id: deploy_id.to_string() };
    let resp = ctx.client.post::<_, Deploy>(&["services", name, "rollback"], &[], &body).await?;
    queued_deploy(ctx, &resp, follow).await
}

pub async fn restart(ctx: &Ctx, name: &str, follow: bool) -> Result<()> {
    let resp = ctx.client.post_empty::<Deploy>(&["services", name, "restart"]).await?;
    queued_deploy(ctx, &resp, follow).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{DeploySource, DeployStatus, DeployTrigger};

    #[test]
    fn commit_summaries() {
        let mut d = Deploy::new("srv-1", DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        assert_eq!(commit_summary(&d), "-");
        d.commit_sha = Some("0123456789abcdef".into());
        assert_eq!(commit_summary(&d), "0123456");
        d.commit_message = Some("Fix the thing\n\nLong body".into());
        assert_eq!(commit_summary(&d), "0123456 Fix the thing");
        d.commit_sha = None;
        assert_eq!(commit_summary(&d), "Fix the thing");
    }

    #[test]
    fn table_lists_deploys() {
        let mut d = Deploy::new("srv-1", DeployTrigger::Upload, DeploySource::Archive { path: "p".into() });
        d.id = "dep-aaaaaaaaaaaaaaaaaaaa".into();
        d.status = DeployStatus::BuildFailed;
        let out = deploys_table(&[d], false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("ID"));
        assert!(lines[1].starts_with("dep-aaaaaaaaaaaaaaaaaaaa   build_failed   upload"));
    }
}
