//! The setup code of a server that has no account yet (DESIGN.md §20).
//!
//! Whoever creates the first account owns the server, so creating it takes
//! a code only someone on the server can read: `ferryd` prints it in a link
//! at startup, and it stays in `<data-dir>/setup_code` (`0600`) until the
//! account exists.

use std::io::Write;
use std::path::{Path, PathBuf};

use ferry_core::{Config, Result, Store};

pub fn code_path(data_dir: &Path) -> PathBuf {
    data_dir.join("setup_code")
}

/// The code a setup must give, while one is expected.
pub fn read_code(data_dir: &Path) -> Option<String> {
    let code = std::fs::read_to_string(code_path(data_dir)).ok()?.trim().to_string();
    (!code.is_empty()).then_some(code)
}

/// Forget the code: the account exists.
pub fn clear_code(data_dir: &Path) {
    let _ = std::fs::remove_file(code_path(data_dir));
}

/// The setup code of this server: the one on disk, or a new one, while it
/// has no account; `None` (and no file) once it has one.
pub async fn ensure_code(config: &Config, store: &Store) -> Result<Option<String>> {
    if store.first_user().await?.is_some() {
        clear_code(&config.data_dir);
        return Ok(None);
    }
    if let Some(code) = read_code(&config.data_dir) {
        return Ok(Some(code));
    }
    let code = ferry_core::ids::random_secret(24);
    let path = code_path(&config.data_dir);
    let _ = std::fs::remove_file(&path); // an empty file
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(&path)?.write_all(code.as_bytes())?;
    Ok(Some(code))
}

/// The page of the dashboard that creates the account, with the code filled in.
pub fn setup_url(base: &str, code: &str) -> String {
    format!("{}/setup?code={code}", base.trim_end_matches('/'))
}
