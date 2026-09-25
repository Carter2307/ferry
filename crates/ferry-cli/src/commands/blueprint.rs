//! `ferry blueprint apply [FILE] [--dry-run]`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ferry_core::dto::{ApplyBlueprint, BlueprintAction, BlueprintResult, ServiceView};

use super::{Ctx, print_json};
use crate::cli::BlueprintApplyArgs;
use crate::output::{self, Color, outln};

/// Default blueprint files, in order of preference.
pub const DEFAULT_FILES: &[&str] = &["ferry.yaml", "render.yaml"];

/// The first default blueprint file that exists in `dir`.
pub fn find_blueprint(dir: &Path) -> Result<PathBuf> {
    for name in DEFAULT_FILES {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    bail!("no blueprint found: expected ./ferry.yaml or ./render.yaml (or pass a FILE)")
}

pub async fn apply(ctx: &Ctx, a: BlueprintApplyArgs) -> Result<()> {
    let file = match a.file {
        Some(f) => f,
        None => {
            let cwd = std::env::current_dir().context("reading the current directory")?;
            let found = tokio::task::spawn_blocking({
                let cwd = cwd.clone();
                move || find_blueprint(&cwd)
            })
            .await
            .context("finding the blueprint")??;
            found.strip_prefix(&cwd).map(Path::to_path_buf).unwrap_or(found)
        }
    };
    let yaml = tokio::fs::read_to_string(&file).await.with_context(|| format!("reading {}", file.display()))?;
    let body = ApplyBlueprint { yaml, dry_run: a.dry_run };
    let resp = ctx.client.post::<_, BlueprintResult>(&["blueprints", "apply"], &[], &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    // Deploys only carry service ids: map them to names for display.
    let names: HashMap<String, String> = if resp.data.deploys.is_empty() {
        HashMap::new()
    } else {
        match ctx.client.get::<Vec<ServiceView>>(&["services"], &[]).await {
            Ok(list) => list.data.into_iter().map(|v| (v.service.id, v.service.name)).collect(),
            Err(_) => HashMap::new(),
        }
    };
    outln!("{}", render_result(&file.display().to_string(), &resp.data, &names, output::stdout_color()).trim_end())?;
    Ok(())
}

fn action_line(a: &BlueprintAction, sign: &str, color: bool, c: Color) -> String {
    let resource = a.resource.replace('_', " ");
    format!("  {} {resource} {}", output::paint(color, c, sign), a.name)
}

/// Human-readable summary of a blueprint apply.
pub fn render_result(file: &str, r: &BlueprintResult, names: &HashMap<String, String>, color: bool) -> String {
    let mut out = String::new();
    let mut push = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    if r.dry_run {
        push(format!("Blueprint {file} — dry run, nothing was changed"));
    } else {
        push(format!("Applied blueprint {file}"));
    }
    let groups = [
        ("create", if r.dry_run { "Would create:" } else { "Created:" }, "+", Color::Green),
        ("update", if r.dry_run { "Would update:" } else { "Updated:" }, "~", Color::Yellow),
        ("unchanged", "Unchanged:", "=", Color::Dim),
    ];
    for (action, title, sign, c) in groups {
        let items: Vec<&BlueprintAction> = r.actions.iter().filter(|a| a.action == action).collect();
        if items.is_empty() {
            continue;
        }
        push(String::new());
        push(output::paint(color, Color::Bold, title));
        for a in items {
            push(action_line(a, sign, color, c));
            for change in &a.changes {
                push(format!("      - {change}"));
            }
        }
    }
    let known = ["create", "update", "unchanged"];
    let others: Vec<&BlueprintAction> = r.actions.iter().filter(|a| !known.contains(&a.action.as_str())).collect();
    if !others.is_empty() {
        push(String::new());
        push(output::paint(color, Color::Bold, "Other:"));
        for a in others {
            push(format!("  {} {} {}", a.action, a.resource.replace('_', " "), a.name));
        }
    }
    if r.actions.is_empty() {
        push(String::new());
        push("The blueprint declares no resources.".to_string());
    }
    if !r.warnings.is_empty() {
        push(String::new());
        push(output::paint(color, Color::Bold, "Warnings:"));
        for w in &r.warnings {
            push(format!("  {} {w}", output::paint(color, Color::Yellow, "!")));
        }
    }
    if !r.deploys.is_empty() {
        push(String::new());
        push(output::paint(color, Color::Bold, "Queued deploys:"));
        for d in &r.deploys {
            let svc = names.get(&d.service_id).cloned().unwrap_or_else(|| d.service_id.clone());
            push(format!("  {}  {svc} ({}, {})", d.id, d.trigger, d.status));
        }
        push(String::new());
        push("Follow a deploy with: ferry logs --deploy <id> -f".to_string());
    } else if r.dry_run && !r.actions.is_empty() {
        push(String::new());
        push("Run without --dry-run to apply.".to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{Deploy, DeploySource, DeployTrigger};

    fn action(resource: &str, name: &str, action: &str, changes: &[&str]) -> BlueprintAction {
        BlueprintAction {
            resource: resource.into(),
            name: name.into(),
            action: action.into(),
            changes: changes.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn result_is_grouped_by_action() {
        let mut d = Deploy::new("srv-1", DeployTrigger::Blueprint, DeploySource::Image { image: "nginx".into() });
        d.id = "dep-1".into();
        let r = BlueprintResult {
            dry_run: false,
            actions: vec![
                action("env_group", "shared", "create", &[]),
                action("service", "web", "update", &["instances: 1 → 2", "image: nginx → caddy"]),
                action("datastore", "db", "unchanged", &[]),
                action("service", "hello", "create", &[]),
            ],
            deploys: vec![d],
            warnings: vec!["services[0].plan is not supported (ignored)".into()],
        };
        let names = HashMap::from([("srv-1".to_string(), "hello".to_string())]);
        let out = render_result("ferry.yaml", &r, &names, false);
        let expected = "\
Applied blueprint ferry.yaml

Created:
  + env group shared
  + service hello

Updated:
  ~ service web
      - instances: 1 → 2
      - image: nginx → caddy

Unchanged:
  = datastore db

Warnings:
  ! services[0].plan is not supported (ignored)

Queued deploys:
  dep-1  hello (blueprint, queued)

Follow a deploy with: ferry logs --deploy <id> -f
";
        assert_eq!(out, expected);
    }

    #[test]
    fn dry_run_wording() {
        let r = BlueprintResult {
            dry_run: true,
            actions: vec![action("service", "web", "create", &[])],
            deploys: vec![],
            warnings: vec![],
        };
        let out = render_result("render.yaml", &r, &HashMap::new(), false);
        assert!(
            out.starts_with("Blueprint render.yaml — dry run, nothing was changed\n\nWould create:\n  + service web\n")
        );
        assert!(out.ends_with("Run without --dry-run to apply.\n"));
    }

    #[test]
    fn default_file_lookup() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_blueprint(dir.path()).is_err());
        std::fs::write(dir.path().join("render.yaml"), "services: []").unwrap();
        assert_eq!(find_blueprint(dir.path()).unwrap(), dir.path().join("render.yaml"));
        std::fs::write(dir.path().join("ferry.yaml"), "services: []").unwrap();
        assert_eq!(find_blueprint(dir.path()).unwrap(), dir.path().join("ferry.yaml"));
    }
}
