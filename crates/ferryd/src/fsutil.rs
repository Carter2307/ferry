//! Filesystem helpers: the private data directory and its secret files.

use std::path::Path;

use anyhow::Context;

/// Make the data dir private (it holds secrets: env vars, datastore
/// passwords, credentialed repo URLs, logs, uploads).
pub fn secure_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating data dir {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting permissions of {}", dir.display()))?;
    }
    Ok(())
}

/// Write a small secret file readable only by the current user.
pub fn write_private_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(contents.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Read a secret file, tightening its permissions if they're too open.
pub fn read_private_file(path: &Path) -> Option<String> {
    let value = std::fs::read_to_string(path).ok()?.trim().to_string();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    (!value.is_empty()).then_some(value)
}
