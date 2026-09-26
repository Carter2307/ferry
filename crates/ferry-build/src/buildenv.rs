//! Cache key of the build-time environment.
//!
//! Generated Dockerfiles receive service env vars as BuildKit secrets, whose
//! values are not part of the layer cache key. A digest of the values is
//! passed as `--build-arg FERRY_BUILD_ENV_DIGEST=…` instead, so changing a
//! variable still re-runs the steps that see it. That build arg is recorded
//! in the image history, so the digest is an HMAC under a per-installation
//! random key (`<repos_dir>/.build-env-key`, mode 0600): it cannot be used to
//! guess low-entropy values offline.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Name of the key file inside the repos directory.
const KEY_FILE: &str = ".build-env-key";
/// Hex characters of key material.
const KEY_LEN: usize = 64;

/// The installation's digest key, loaded (or created) on first use.
#[derive(Clone)]
pub(crate) struct EnvKey {
    path: PathBuf,
    key: Arc<tokio::sync::OnceCell<Vec<u8>>>,
}

impl std::fmt::Debug for EnvKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvKey").field("path", &self.path).finish_non_exhaustive()
    }
}

impl EnvKey {
    pub(crate) fn new(repos_dir: &Path) -> EnvKey {
        EnvKey { path: repos_dir.join(KEY_FILE), key: Arc::new(tokio::sync::OnceCell::new()) }
    }

    /// HMAC-SHA256 of `env` (order-independent), 32 hex characters.
    pub(crate) async fn digest(&self, env: &[(String, String)]) -> String {
        let key = self
            .key
            .get_or_init(|| async {
                let path = self.path.clone();
                match tokio::task::spawn_blocking(move || load_or_create(&path)).await {
                    Ok(Ok(key)) => key,
                    Ok(Err(e)) => {
                        tracing::warn!(path = %self.path.display(), "cannot persist the build env key ({e}); using a temporary one");
                        ferry_core::ids::random_secret(KEY_LEN).into_bytes()
                    }
                    Err(e) => {
                        tracing::warn!("loading the build env key failed: {e}; using a temporary one");
                        ferry_core::ids::random_secret(KEY_LEN).into_bytes()
                    }
                }
            })
            .await;
        digest(key, env)
    }
}

/// Read the key file, or create it atomically (a complete file appears under
/// its final name, whoever wins a race keeps it).
fn load_or_create(path: &Path) -> io::Result<Vec<u8>> {
    if let Some(key) = read_key(path)? {
        return Ok(key);
    }
    let dir = path.parent().ok_or_else(|| io::Error::other("key path has no parent"))?;
    fs::create_dir_all(dir)?;
    let key = ferry_core::ids::random_secret(KEY_LEN);
    let tmp = dir.join(format!("{KEY_FILE}.{}.tmp", ferry_core::ids::random_secret(8)));
    let written = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(key.as_bytes())?;
        f.sync_all()?;
        // Fails when another build created the key meanwhile: use theirs.
        fs::hard_link(&tmp, path)
    })();
    let _ = fs::remove_file(&tmp);
    match written {
        Ok(()) => Ok(key.into_bytes()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            read_key(path)?.ok_or_else(|| io::Error::other("the build env key file is invalid"))
        }
        Err(e) => Err(e),
    }
}

/// `Ok(None)` when the file is missing; an unusable file is replaced.
fn read_key(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut contents = String::new();
    match fs::File::open(path) {
        Ok(mut f) => {
            f.read_to_string(&mut contents)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    let key = contents.trim();
    if key.len() >= 32 {
        return Ok(Some(key.as_bytes().to_vec()));
    }
    tracing::warn!(path = %path.display(), "ignoring an invalid build env key file");
    fs::remove_file(path)?;
    Ok(None)
}

fn digest(key: &[u8], env: &[(String, String)]) -> String {
    let mut pairs: Vec<&(String, String)> = env.iter().collect();
    pairs.sort();
    // Invariant: HMAC accepts keys of any length.
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else { return String::new() };
    for (k, v) in pairs {
        for part in [k.as_bytes(), v.as_bytes()] {
            mac.update(&(part.len() as u64).to_le_bytes());
            mac.update(part);
        }
    }
    mac.finalize().into_bytes().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn digest_is_keyed_and_order_independent() {
        let a = digest(b"key-one-key-one-key-one-key-one!", &env(&[("A", "1"), ("B", "2")]));
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(a, digest(b"key-one-key-one-key-one-key-one!", &env(&[("B", "2"), ("A", "1")])));
        assert_ne!(a, digest(b"key-two-key-two-key-two-key-two!", &env(&[("A", "1"), ("B", "2")])));
        assert_ne!(a, digest(b"key-one-key-one-key-one-key-one!", &env(&[("A", "1"), ("B", "3")])));
        // No ambiguity between key and value boundaries.
        assert_ne!(digest(b"k", &env(&[("AB", "C")])), digest(b"k", &env(&[("A", "BC")])));
    }

    #[tokio::test]
    async fn key_is_persisted_privately_and_reused() {
        let d = tempfile::tempdir().unwrap();
        let repos = d.path().join("repos");
        let e = env(&[("SECRET", "hunter2")]);
        let first = EnvKey::new(&repos).digest(&e).await;
        let path = repos.join(KEY_FILE);
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Another builder (e.g. after a restart) computes the same digest.
        assert_eq!(EnvKey::new(&repos).digest(&e).await, first);
        assert!(!format!("{:?}", EnvKey::new(&repos)).contains(&fs::read_to_string(&path).unwrap()));
        // No temp files left behind.
        assert_eq!(fs::read_dir(&repos).unwrap().count(), 1);
        // A damaged key file is replaced.
        fs::write(&path, "short").unwrap();
        let other = EnvKey::new(&repos).digest(&e).await;
        assert_ne!(other, first);
        assert_eq!(fs::read_to_string(&path).unwrap().len(), KEY_LEN);
    }
}
