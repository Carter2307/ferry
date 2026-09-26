//! `ferry blueprint apply [FILE] [--dry-run]`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ferry_core::dto::{ApplyBlueprint, BlueprintAction, BlueprintResult, ServiceView};
use serde_yaml::Value;

use super::{Ctx, print_json};
use crate::cli::BlueprintApplyArgs;
use crate::output::{self, Color, errln, outln};
use crate::repo;

/// Default blueprint files, in order of preference.
pub const DEFAULT_FILES: &[&str] = &["ferry.yaml", "render.yaml"];

/// Rewrite every service's relative local `repo:` path that exists relative
/// to `base` (the blueprint's directory) as an absolute path: the server
/// clones it, and would resolve a relative path against its own working
/// directory. Returns the YAML to send (the original text, byte for byte,
/// when nothing changes) and one note per rewritten repo.
pub fn absolutize_repos(yaml: &str, base: &Path) -> (String, Vec<String>) {
    let unchanged = || (yaml.to_string(), Vec::new());
    // Invalid YAML is sent as is: the server reports the error in context.
    let Ok(mut root) = serde_yaml::from_str::<Value>(yaml) else { return unchanged() };
    if root.apply_merge().is_err() {
        return unchanged();
    }
    let mut notes = Vec::new();
    let mut fix = |services: Option<&mut Value>| {
        let Some(Value::Sequence(services)) = services else { return };
        for svc in services {
            let name = svc.get("name").and_then(Value::as_str).unwrap_or("?").to_string();
            let Some(Value::String(r)) = svc.get_mut("repo") else { continue };
            if let Some(abs) = repo::absolutize(r, base) {
                notes.push(format!("service '{name}': repo '{}' → {abs}", r.trim()));
                *r = abs;
            }
        }
    };
    fix(root.get_mut("services"));
    if let Some(Value::Sequence(projects)) = root.get_mut("projects") {
        for project in projects {
            if let Some(Value::Sequence(envs)) = project.get_mut("environments") {
                for env in envs {
                    fix(env.get_mut("services"));
                }
            }
        }
    }
    if notes.is_empty() {
        return unchanged();
    }
    match serde_yaml::to_string(&root) {
        Ok(text) => (text, notes),
        Err(_) => unchanged(),
    }
}

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
    // Local repo paths are relative to the blueprint file, not to ferryd.
    let base = match file.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let (yaml, notes) =
        tokio::task::spawn_blocking(move || absolutize_repos(&yaml, &base)).await.context("reading the blueprint")?;
    for note in notes {
        errln!("note: {note}");
    }
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
    fn relative_repo_paths_become_absolute() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mono/worker")).unwrap();
        std::fs::create_dir_all(dir.path().join("echo")).unwrap();
        let mono = std::fs::canonicalize(dir.path().join("mono")).unwrap();
        let echo = std::fs::canonicalize(dir.path().join("echo")).unwrap();
        let yaml = "\
# comments are fine
services:
  - type: worker
    name: relrepo
    repo: ./mono
    rootDir: worker
    runtime: python
    startCommand: python worker.py
  - type: web
    name: remote
    repo: https://github.com/a/b
  - type: web
    name: missing
    repo: ./not-here
  - type: web
    name: img
    image: { url: nginx:alpine }
projects:
  - name: p
    environments:
      - name: prod
        services:
          - { type: web, name: nested, repo: echo, branch: main }
";
        let (out, notes) = absolutize_repos(yaml, dir.path());
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].starts_with("service 'relrepo': repo './mono' → "), "{notes:?}");
        let v: Value = serde_yaml::from_str(&out).unwrap();
        let svc = |i: usize| &v["services"][i];
        assert_eq!(svc(0)["repo"].as_str(), mono.to_str());
        assert_eq!(svc(0)["rootDir"], "worker");
        assert_eq!(svc(0)["startCommand"], "python worker.py");
        assert_eq!(svc(1)["repo"], "https://github.com/a/b");
        assert_eq!(svc(2)["repo"], "./not-here", "left for the server to reject");
        assert_eq!(svc(3)["image"]["url"], "nginx:alpine");
        let nested = &v["projects"][0]["environments"][0]["services"][0];
        assert_eq!(nested["repo"].as_str(), echo.to_str());
        assert_eq!(nested["branch"], "main");

        // Nothing to rewrite: the original text is sent untouched.
        let plain = "services:\n  - {type: web, name: a, repo: 'https://x/y'}  # keep me\n";
        assert_eq!(absolutize_repos(plain, dir.path()), (plain.to_string(), vec![]));
        let broken = "services: [\n";
        assert_eq!(absolutize_repos(broken, dir.path()).0, broken);
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
