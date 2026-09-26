//! Local repository paths.
//!
//! The server clones `repo_url` itself, so a relative path would resolve
//! against ferryd's working directory — not what the user typed. The server
//! rejects relative paths; the CLI turns a relative path that exists locally
//! (relative to the current directory, or to the blueprint file) into an
//! absolute one before sending it.

use std::path::{Path, PathBuf};

/// Is `raw` a relative filesystem path, i.e. not a URL (`scheme://…`), an
/// scp-style address (`user@host:path`) or an absolute path? Mirrors the
/// shapes accepted by `ferry_core::validate::repo_url`.
pub fn is_relative_path(raw: &str) -> bool {
    let u = raw.trim();
    if u.is_empty() || u.contains("://") || u.starts_with('/') {
        return false;
    }
    if let Some(colon) = u.find(':') {
        let host = &u[..colon];
        if !host.is_empty() && !host.contains('/') && colon + 1 < u.len() {
            return false; // scp-like: [user@]host:path
        }
    }
    true
}

/// `~/x` → `$HOME/x` (the shell doesn't expand quoted or `--repo=~/x` values).
fn expand_home(raw: &str, home: Option<PathBuf>) -> Option<PathBuf> {
    let rest = raw.strip_prefix("~/").or_else(|| (raw == "~").then_some(""))?;
    home.filter(|h| h.is_absolute()).map(|h| h.join(rest))
}

/// When `raw` is a relative (or `~/`) path of an existing local directory,
/// resolved against `base`, its canonical absolute path. `None` for URLs,
/// absolute paths and paths that don't exist here.
pub fn absolutize(raw: &str, base: &Path) -> Option<String> {
    let trimmed = raw.trim();
    let candidate = match expand_home(trimmed, std::env::home_dir()) {
        Some(p) => p,
        None if is_relative_path(trimmed) => base.join(trimmed),
        None => return None,
    };
    if !candidate.is_dir() {
        return None;
    }
    std::fs::canonicalize(candidate).ok()?.to_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_told_apart_from_urls() {
        for rel in ["./git/mono", "../echo", "mono", "a/b", "./a:b", ".", "-weird"] {
            assert!(is_relative_path(rel), "{rel}");
        }
        for not_rel in [
            "",
            "  ",
            "/srv/git/app",
            "file:///srv/git/app",
            "https://github.com/a/b.git",
            "ssh://git@github.com/a/b",
            "git@github.com:a/b.git",
            "host:repo",
        ] {
            assert!(!is_relative_path(not_rel), "{not_rel}");
        }
    }

    #[test]
    fn existing_relative_directories_become_absolute() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("git/mono")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        let canon = std::fs::canonicalize(dir.path().join("git/mono")).unwrap();
        let want = canon.to_str().unwrap();
        assert_eq!(absolutize("./git/mono", dir.path()).as_deref(), Some(want));
        assert_eq!(absolutize(" git/mono/ ", dir.path()).as_deref(), Some(want));
        assert_eq!(absolutize("git/../git/mono", dir.path()).as_deref(), Some(want));
        // Not a local directory, or not a relative path: left alone.
        assert_eq!(absolutize("./missing", dir.path()), None);
        assert_eq!(absolutize("file.txt", dir.path()), None);
        assert_eq!(absolutize("https://github.com/a/b", dir.path()), None);
        assert_eq!(absolutize("git@github.com:a/b.git", dir.path()), None);
        assert_eq!(absolutize(want, dir.path()), None, "already absolute");
    }

    #[test]
    fn home_relative_paths_expand() {
        let home = PathBuf::from("/home/me");
        assert_eq!(expand_home("~/code/app", Some(home.clone())), Some(home.join("code/app")));
        assert_eq!(expand_home("~", Some(home.clone())), Some(home.join("")));
        assert_eq!(expand_home("~other/x", Some(home.clone())), None);
        assert_eq!(expand_home("./x", Some(home)), None);
        assert_eq!(expand_home("~/x", None), None);
    }
}
