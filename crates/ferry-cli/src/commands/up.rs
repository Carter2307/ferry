//! `ferry up`: pack a local directory, create the service if needed (or
//! apply the given settings to the existing one), upload the archive and
//! deploy it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ferry_core::dto::{LinkEnvGroup, PatchEnv, ServiceView};
use ferry_core::{Deploy, EnvVar, ServiceType, SourceKind, validate};

use super::services::{create_body, settings_labels, settings_update};
use super::{Ctx, queued_deploy};
use crate::archive::{self, SkippedLink};
use crate::cli::UpArgs;
use crate::client::is_not_found;
use crate::output::{self, errln, outln};

/// Server-side upload limit (DESIGN.md §10).
pub const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// Turn an arbitrary directory name into a service name candidate:
/// lowercase ASCII letters/digits with single `-` separators, starting with a
/// letter, at most 40 characters. May return an empty string.
pub fn sanitize_service_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let start = out.find(|c: char| c.is_ascii_lowercase()).unwrap_or(out.len());
    let mut name: String = out[start..].chars().take(40).collect();
    while name.ends_with('-') {
        name.pop();
    }
    name
}

/// Default service name for a directory (its sanitized name), validated.
pub fn default_service_name(dir: &Path) -> Result<String> {
    let raw = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let name = sanitize_service_name(raw);
    if name.is_empty() {
        bail!("cannot derive a service name from directory '{}': pass one, e.g. 'ferry up my-app'", dir.display());
    }
    if let Err(e) = validate::resource_name(&name) {
        // Reserved (`ferry`) or id-shaped (`srv-<hex>`) names.
        bail!(
            "directory name '{raw}' gives the service name '{name}', which can't be used ({e}): \
             pass another name, e.g. 'ferry up {name}-app'"
        );
    }
    Ok(name)
}

/// Resolve `--dir` to an absolute directory path.
async fn resolve_dir(dir: &Path) -> Result<PathBuf> {
    let abs = tokio::fs::canonicalize(dir).await.with_context(|| format!("directory {} not found", dir.display()))?;
    let meta = tokio::fs::metadata(&abs).await.with_context(|| format!("reading {}", abs.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(abs)
}

/// `--type` can only pick the type of a new service.
fn check_type(a: &UpArgs, existing: &ServiceView) -> Result<()> {
    let current = existing.service.service_type;
    match a.service_type {
        Some(wanted) if wanted != current => bail!(
            "service '{}' is a {}; the type of an existing service can't change (--type {} only applies when \
             creating a service): delete it first, or deploy under another name",
            existing.service.name,
            output::type_long(current),
            output::type_short(wanted)
        ),
        _ => Ok(()),
    }
}

/// Apply `ferry up`'s settings, variables and env groups to an existing
/// service before deploying, so re-running `ferry up --start-cmd …` fixes it.
async fn update_existing(ctx: &Ctx, mut svc: ServiceView, a: &UpArgs) -> Result<ServiceView> {
    let id = svc.service.id.clone();
    let mut applied: Vec<String> = Vec::new();
    if !a.settings.is_empty() {
        let body = settings_update(&a.settings);
        svc = ctx.client.patch::<_, ServiceView>(&["services", &id], &[], &body).await?.data;
        applied.extend(settings_labels(&a.settings).into_iter().map(str::to_string));
    }
    if !a.env.is_empty() {
        // No restart: the deploy below starts with the new environment.
        let body = PatchEnv { set: a.env.clone(), unset: Vec::new() };
        let query = [("restart", "false".to_string())];
        ctx.client.patch::<_, Vec<EnvVar>>(&["services", &id, "env"], &query, &body).await?;
        let keys: Vec<&str> = a.env.iter().map(|v| v.key.as_str()).collect();
        applied.push(format!("env {}", keys.join(", ")));
    }
    for group in &a.env_groups {
        if svc.env_groups.iter().any(|g| g == group) {
            continue;
        }
        let body = LinkEnvGroup { group: group.clone() };
        svc = ctx.client.post::<_, ServiceView>(&["services", &id, "env-groups"], &[], &body).await?.data;
        applied.push(format!("env group {group}"));
    }
    if !applied.is_empty() {
        errln!("Updated '{}': {}", svc.service.name, applied.join("; "));
    }
    Ok(svc)
}

/// One line about symlinks left out of the archive.
fn skipped_links_warning(dir: &Path, skipped: &[SkippedLink]) -> Option<String> {
    const SHOWN: usize = 3;
    if skipped.is_empty() {
        return None;
    }
    let list: Vec<String> = skipped.iter().take(SHOWN).map(|l| format!("{} -> {}", l.rel, l.target)).collect();
    let more = if skipped.len() > SHOWN { format!(", and {} more", skipped.len() - SHOWN) } else { String::new() };
    Some(format!(
        "warning: not uploading {} symlink(s) pointing outside {} (the server can't accept them): {}{more}",
        skipped.len(),
        dir.display(),
        list.join(", ")
    ))
}

pub async fn up(ctx: &Ctx, a: UpArgs) -> Result<()> {
    let dir = resolve_dir(&a.dir).await?;
    let name = match &a.name {
        Some(n) => n.clone(),
        None => default_service_name(&dir)?,
    };

    // Look the service up first so a typo'd name fails before packing.
    let existing = match ctx.client.get::<ServiceView>(&["services", &name], &[]).await {
        Ok(v) => Some(v.data),
        Err(e) if is_not_found(&e) => None,
        Err(e) => return Err(e),
    };
    match &existing {
        Some(svc) => check_type(&a, svc)?,
        // Validate locally: the name will be used to create the service.
        None => validate::resource_name(&name).map_err(|e| anyhow!("{e}"))?,
    }

    errln!("Packing {} …", dir.display());
    let pack_dir = dir.clone();
    let cancel = archive::Cancel::default();
    // If this future is dropped (Ctrl-C), stop packing so the temp file goes away.
    let _stop_packing = archive::CancelOnDrop(cancel.clone());
    let packed = tokio::task::spawn_blocking(move || archive::pack_dir(&pack_dir, &cancel))
        .await
        .context("packing the directory")??;
    if packed.files == 0 {
        bail!("nothing to upload: {} has no files (after .gitignore/.ferryignore rules)", dir.display());
    }
    errln!("Packed {} file(s), {}", packed.files, output::human_bytes(packed.size));
    if let Some(warning) = skipped_links_warning(&dir, &packed.skipped) {
        errln!("{warning}");
    }
    if packed.size > MAX_UPLOAD_BYTES {
        bail!(
            "the archive is {}, over the server's {} upload limit: exclude large files with a .ferryignore",
            output::human_bytes(packed.size),
            output::human_bytes(MAX_UPLOAD_BYTES)
        );
    }

    let service = match existing {
        Some(svc) => {
            let svc = update_existing(ctx, svc, &a).await?;
            match svc.service.source_kind() {
                SourceKind::Git => errln!(
                    "note: '{}' normally builds from {}; this deploy uses your local files instead",
                    svc.service.name,
                    svc.service.repo_url.as_deref().unwrap_or("git")
                ),
                SourceKind::Image => errln!(
                    "note: '{}' normally runs the image {}; this deploy builds your local files instead",
                    svc.service.name,
                    svc.service.image.as_deref().unwrap_or("?")
                ),
                SourceKind::Upload => {}
            }
            svc
        }
        None => {
            let mut body = create_body(&name, Some(a.service_type.unwrap_or(ServiceType::WebService)), &a.settings);
            body.env = (!a.env.is_empty()).then(|| a.env.clone());
            body.env_groups = (!a.env_groups.is_empty()).then(|| a.env_groups.clone());
            body.deploy = Some(false);
            let created = ctx.client.post::<_, ServiceView>(&["services"], &[], &body).await?.data;
            errln!(
                "Created {} '{}' ({})",
                output::type_long(created.service.service_type),
                created.service.name,
                created.service.id
            );
            created
        }
    };

    errln!("Uploading {} …", output::human_bytes(packed.size));
    let query: Vec<(&str, String)> = if a.clear_cache { vec![("clear_cache", "true".to_string())] } else { Vec::new() };
    let deploy = ctx
        .client
        .upload::<Deploy>(&["services", &service.service.id, "deploys", "upload"], &query, packed.path())
        .await?;
    drop(packed); // delete the temp archive before a potentially long follow
    if !ctx.json
        && !a.follow
        && let Some(url) = &service.url
    {
        outln!("URL: {url}")?;
    }
    queued_deploy(ctx, &deploy, a.follow).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_directory_names() {
        assert_eq!(sanitize_service_name("my-app"), "my-app");
        assert_eq!(sanitize_service_name("My_App.v2"), "my-app-v2");
        assert_eq!(sanitize_service_name("  hello   world  "), "hello-world");
        assert_eq!(sanitize_service_name("123-app"), "app");
        assert_eq!(sanitize_service_name("--x--"), "x");
        assert_eq!(sanitize_service_name("café"), "caf");
        assert_eq!(sanitize_service_name("日本"), "");
        assert_eq!(sanitize_service_name("2024"), "");
        let long = "a".repeat(39) + "-bcdef";
        assert_eq!(sanitize_service_name(&long), "a".repeat(39));
        assert_eq!(sanitize_service_name(&"x".repeat(60)).len(), 40);
    }

    #[test]
    fn sanitized_names_are_valid_resource_names() {
        for raw in ["My Project", "api_server", "Node.JS-App!", "a", "x-1", "UPPER", "v2.0.1-beta"] {
            let n = sanitize_service_name(raw);
            assert!(validate::resource_name(&n).is_ok(), "{raw} -> {n}");
        }
    }

    #[test]
    fn default_names_from_directories() {
        assert_eq!(default_service_name(Path::new("/home/me/My_Site")).unwrap(), "my-site");
        // "api" is an ordinary name now; only "ferry" and "localhost" are reserved.
        assert_eq!(default_service_name(Path::new("/srv/api")).unwrap(), "api");
        let err = default_service_name(Path::new("/srv/ferry")).unwrap_err().to_string();
        assert!(err.contains("reserved") && err.contains("ferry-app"), "{err}");
        assert!(default_service_name(Path::new("/tmp/123")).is_err());
        assert!(default_service_name(Path::new("/")).is_err());
    }

    #[test]
    fn skipped_links_are_summarized() {
        let dir = Path::new("/w/site");
        assert_eq!(skipped_links_warning(dir, &[]), None);
        let link = |i: usize| SkippedLink { rel: format!("venv/bin/l{i}"), target: format!("/usr/bin/t{i}") };
        let one = skipped_links_warning(dir, &[link(1)]).unwrap();
        assert_eq!(
            one,
            "warning: not uploading 1 symlink(s) pointing outside /w/site (the server can't accept them): \
             venv/bin/l1 -> /usr/bin/t1"
        );
        let many = skipped_links_warning(dir, &(1..=5).map(link).collect::<Vec<_>>()).unwrap();
        assert!(many.contains("5 symlink(s)") && many.ends_with("venv/bin/l3 -> /usr/bin/t3, and 2 more"), "{many}");
    }
}
