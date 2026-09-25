//! Filesystem helpers: user-supplied relative paths, scratch directories.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Validate an id used as a single path component (`<builds_dir>/<deploy_id>`).
pub(crate) fn safe_component(kind: &str, s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s != "."
        && s != ".."
        && s.len() <= 128
        && !s.chars().any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control());
    if ok { Ok(()) } else { Err(format!("invalid {kind} '{s}'")) }
}

/// Normalize a user-supplied path that must be relative and stay inside its
/// base: strips `./`, resolves inner `..` lexically, rejects absolute paths
/// and escapes. `""`, `"."` and `"./"` yield an empty path (the base itself).
pub(crate) fn normalize_relative(what: &str, p: &str) -> Result<PathBuf, String> {
    let trimmed = p.trim();
    let path = Path::new(trimmed);
    let mut out: Vec<&std::ffi::OsStr> = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.pop().is_none() {
                    return Err(format!("{what} '{trimmed}' must not point outside the source directory"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{what} '{trimmed}' must be a relative path"));
            }
        }
    }
    Ok(out.iter().collect())
}

/// Join a normalized relative path onto `base` and make sure the real
/// (symlink-resolved) location stays inside `boundary`. Returns the canonical
/// path. `Ok(None)` when it does not exist.
pub(crate) fn resolve_inside(base: &Path, rel: &Path, boundary: &Path) -> Result<Option<PathBuf>, String> {
    let joined = base.join(rel);
    match fs::canonicalize(&joined) {
        Ok(canon) => {
            let boundary = fs::canonicalize(boundary).map_err(|e| format!("{}: {e}", boundary.display()))?;
            if canon.starts_with(&boundary) {
                Ok(Some(canon))
            } else {
                Err(format!("'{}' resolves outside the source directory", rel.display()))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("'{}': {e}", rel.display())),
    }
}

/// Resolve the build context: `root_dir` (if any) inside `export_root`.
pub(crate) fn resolve_root_dir(export_root: &Path, root_dir: Option<&str>) -> Result<PathBuf, String> {
    let rel = match root_dir {
        Some(r) if !r.trim().is_empty() => normalize_relative("root directory", r)?,
        _ => PathBuf::new(),
    };
    let display = if rel.as_os_str().is_empty() { ".".to_string() } else { rel.display().to_string() };
    match resolve_inside(export_root, &rel, export_root)? {
        Some(p) if p.is_dir() => Ok(p),
        Some(_) => Err(format!("root directory '{display}' is not a directory")),
        None => Err(format!("root directory '{display}' does not exist in the source")),
    }
}

/// `remove_dir_all` that also copes with read-only directories and treats a
/// missing directory as success.
pub(crate) fn remove_dir_all_force(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(first) => {
            make_writable(dir);
            match fs::remove_dir_all(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(first),
            }
        }
    }
}

#[cfg(unix)]
fn make_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let _ = fs::set_permissions(&d, fs::Permissions::from_mode(0o755));
        if let Ok(rd) = fs::read_dir(&d) {
            for entry in rd.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(entry.path());
                }
            }
        }
    }
}

#[cfg(not(unix))]
fn make_writable(_dir: &Path) {}

/// Async-friendly removal (runs on the blocking pool).
pub(crate) async fn remove_dir_async(dir: PathBuf) -> io::Result<()> {
    match tokio::task::spawn_blocking(move || remove_dir_all_force(&dir)).await {
        Ok(r) => r,
        Err(e) => Err(io::Error::other(format!("removal task failed: {e}"))),
    }
}

/// A scratch directory removed when the build ends — explicitly via
/// [`ScratchDir::cleanup`], or on drop if the build future is abandoned.
pub(crate) struct ScratchDir {
    path: PathBuf,
    armed: bool,
}

impl ScratchDir {
    /// Create `path` from scratch (removing leftovers of a crashed build).
    pub(crate) async fn create(path: PathBuf) -> io::Result<ScratchDir> {
        let p = path.clone();
        let res = tokio::task::spawn_blocking(move || -> io::Result<()> {
            remove_dir_all_force(&p)?;
            fs::create_dir_all(&p)
        })
        .await;
        match res {
            Ok(Ok(())) => Ok(ScratchDir { path, armed: true }),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(io::Error::other(format!("scratch dir task failed: {e}"))),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) async fn cleanup(mut self) {
        self.armed = false;
        let path = self.path.clone();
        if remove_dir_async(path.clone()).await.is_ok() {
            return;
        }
        // A just-canceled extraction may still have been writing: retry once.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Err(e) = remove_dir_async(path.clone()).await {
            tracing::warn!("could not remove build directory {}: {e}", path.display());
        }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let path = std::mem::take(&mut self.path);
        match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                h.spawn_blocking(move || {
                    if let Err(e) = remove_dir_all_force(&path) {
                        tracing::warn!("could not remove build directory {}: {e}", path.display());
                    }
                });
            }
            Err(_) => {
                let _ = remove_dir_all_force(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_paths() {
        assert_eq!(normalize_relative("x", "./api/").unwrap(), PathBuf::from("api"));
        assert_eq!(normalize_relative("x", "a/b/../c").unwrap(), PathBuf::from("a/c"));
        assert_eq!(normalize_relative("x", ".").unwrap(), PathBuf::new());
        assert_eq!(normalize_relative("x", "").unwrap(), PathBuf::new());
        assert!(normalize_relative("x", "../api").unwrap_err().contains("outside"));
        assert!(normalize_relative("x", "a/../../b").is_err());
        assert!(normalize_relative("x", "/etc").unwrap_err().contains("relative"));
    }

    #[test]
    fn root_dir_resolution() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("services/api")).unwrap();
        fs::write(d.path().join("file.txt"), "x").unwrap();
        let root = resolve_root_dir(d.path(), Some("./services/api")).unwrap();
        assert!(root.ends_with("services/api"));
        assert_eq!(resolve_root_dir(d.path(), None).unwrap(), fs::canonicalize(d.path()).unwrap());
        assert!(resolve_root_dir(d.path(), Some("missing")).unwrap_err().contains("does not exist"));
        assert!(resolve_root_dir(d.path(), Some("file.txt")).unwrap_err().contains("not a directory"));
        assert!(resolve_root_dir(d.path(), Some("../")).is_err());
        assert!(resolve_root_dir(d.path(), Some("/tmp")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn root_dir_symlink_escape_is_rejected() {
        let outside = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("escape")).unwrap();
        let err = resolve_root_dir(d.path(), Some("escape")).unwrap_err();
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn components() {
        assert!(safe_component("id", "dep-123").is_ok());
        assert!(safe_component("id", "../x").is_err());
        assert!(safe_component("id", "..").is_err());
        assert!(safe_component("id", "").is_err());
        assert!(safe_component("id", "a/b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn force_removal_of_read_only_tree() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let ro = d.path().join("tree/ro");
        fs::create_dir_all(&ro).unwrap();
        fs::write(ro.join("f"), "x").unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
        remove_dir_all_force(&d.path().join("tree")).unwrap();
        assert!(!d.path().join("tree").exists());
        remove_dir_all_force(&d.path().join("tree")).unwrap();
    }

    #[tokio::test]
    async fn scratch_dir_is_removed_on_cleanup_and_drop() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("dep-1");
        fs::create_dir_all(p.join("leftover")).unwrap();
        let s = ScratchDir::create(p.clone()).await.unwrap();
        assert!(p.exists() && !p.join("leftover").exists());
        s.cleanup().await;
        assert!(!p.exists());

        let s = ScratchDir::create(p.clone()).await.unwrap();
        drop(s);
        for _ in 0..50 {
            if !p.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!p.exists());
    }
}
