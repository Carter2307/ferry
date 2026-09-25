//! `ferry up`: pack a local directory, create the service if needed, upload
//! the archive and deploy it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ferry_core::dto::ServiceView;
use ferry_core::{Deploy, ServiceType, SourceKind, validate};

use super::services::create_body;
use super::{Ctx, queued_deploy};
use crate::archive;
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
    if validate::resource_name(&name).is_err() {
        bail!(
            "directory name '{raw}' gives the service name '{name}', which is reserved: \
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

fn has_creation_flags(a: &UpArgs) -> bool {
    a.service_type.is_some() || !a.settings.is_empty() || !a.env.is_empty() || !a.env_groups.is_empty()
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
    if existing.is_none() {
        // Validate locally: the name will be used to create the service.
        validate::resource_name(&name).map_err(|e| anyhow!("{e}"))?;
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
    if packed.size > MAX_UPLOAD_BYTES {
        bail!(
            "the archive is {}, over the server's {} upload limit: exclude large files with a .ferryignore",
            output::human_bytes(packed.size),
            output::human_bytes(MAX_UPLOAD_BYTES)
        );
    }

    let service = match existing {
        Some(svc) => {
            if has_creation_flags(&a) {
                errln!(
                    "note: service '{}' already exists; creation flags are ignored (use 'ferry update' / 'ferry env set')",
                    svc.service.name
                );
            }
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
        let err = default_service_name(Path::new("/srv/api")).unwrap_err().to_string();
        assert!(err.contains("reserved") && err.contains("api-app"), "{err}");
        assert!(default_service_name(Path::new("/tmp/123")).is_err());
        assert!(default_service_name(Path::new("/")).is_err());
    }
}
