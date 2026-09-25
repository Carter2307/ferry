//! On-disk layout under `certs_dir` (all functions are blocking; call them
//! from `spawn_blocking` in async code):
//!
//! ```text
//! certs_dir/
//!   accounts/<sanitized-directory-url>.json   ACME account credentials (0600)
//!   <host>/cert.pem                           full chain, leaf first
//!   <host>/key.pem                            private key (0600)
//!   <host>/meta.json                          {"issued_at": …, "not_after": …}
//! ```
//!
//! Files are written atomically (temp file + rename) so a crash never leaves
//! a truncated certificate or key behind.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rustls::crypto::CryptoProvider;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::certs::{self, IssuedCert};

pub(crate) const ACCOUNTS_DIR: &str = "accounts";
pub(crate) const CERT_FILE: &str = "cert.pem";
pub(crate) const KEY_FILE: &str = "key.pem";
pub(crate) const META_FILE: &str = "meta.json";

/// Contents of `<host>/meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CertMeta {
    pub issued_at: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    /// ACME directory that issued the certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_url: Option<String>,
}

/// `accounts/<name>.json` for an ACME directory URL: scheme dropped, every
/// character outside `[A-Za-z0-9.-]` replaced by `_`, so staging and
/// production accounts never mix.
pub(crate) fn account_path(certs_dir: &Path, directory_url: &str) -> PathBuf {
    let without_scheme = directory_url.split_once("://").map_or(directory_url, |(_, rest)| rest);
    let mut name: String = without_scheme
        .trim_end_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    name.truncate(150);
    let name = name.trim_start_matches('.');
    let name = if name.is_empty() { "default" } else { name };
    certs_dir.join(ACCOUNTS_DIR).join(format!("{name}.json"))
}

/// Directory holding the certificate of `host` (which must be a validated,
/// normalized domain name, so it can't escape `certs_dir`).
pub(crate) fn host_dir(certs_dir: &Path, host: &str) -> PathBuf {
    certs_dir.join(host)
}

/// Create a directory (and parents) readable only by the owner.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Atomically replace `path` with `contents`. `private` files get mode 0600.
pub(crate) fn write_atomic(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other(format!("{} has no parent directory", path.display())))?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{file_name}.tmp-{}", ferry_core::ids::random_secret(8)));
    let result = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(if private { 0o600 } else { 0o644 });
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut file = opts.open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Persist an issued certificate: key first, then chain, then metadata. A
/// crash in between leaves a key/cert mismatch that [`load_all`] rejects
/// (the certificate is then simply requested again).
pub(crate) fn save_certificate(
    certs_dir: &Path,
    host: &str,
    chain_pem: &str,
    key_pem: &str,
    meta: &CertMeta,
) -> io::Result<()> {
    let dir = host_dir(certs_dir, host);
    create_private_dir(&dir)?;
    write_atomic(&dir.join(KEY_FILE), key_pem.as_bytes(), true)?;
    write_atomic(&dir.join(CERT_FILE), chain_pem.as_bytes(), false)?;
    let meta = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
    write_atomic(&dir.join(META_FILE), &meta, false)
}

/// Load one host directory.
fn load_host(dir: &Path, host: &str, provider: &CryptoProvider) -> Result<IssuedCert, String> {
    let read = |name: &str| fs::read(dir.join(name)).map_err(|e| format!("reading {name}: {e}"));
    let chain_pem = read(CERT_FILE)?;
    let key_pem = read(KEY_FILE)?;
    let meta: Option<CertMeta> = match fs::read(dir.join(META_FILE)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                warn!("ignoring invalid {}: {e}", dir.join(META_FILE).display());
                None
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            warn!("cannot read {}: {e}", dir.join(META_FILE).display());
            None
        }
    };
    let mut issued = certs::issued_from_pem(&chain_pem, &key_pem, meta.as_ref().map(|m| m.issued_at), provider)
        .map_err(|e| e.to_string())?;
    issued.directory_url = meta.as_ref().and_then(|m| m.directory_url.clone());
    let names = certs::dns_names(&issued.key.cert[0]).map_err(|e| e.to_string())?;
    if !certs::covers(&names, host) {
        return Err(format!("the certificate is for {names:?}, not {host}"));
    }
    // The certificate itself is authoritative; meta.json only fills gaps.
    if let Some(m) = &meta
        && m.not_after != issued.not_after
    {
        debug!("{}: meta.json not_after differs from the certificate; using the certificate", dir.display());
    }
    Ok(issued)
}

/// Load every `<host>/` directory under `certs_dir`, skipping (with a
/// warning) entries that are incomplete or corrupt. A missing `certs_dir`
/// yields nothing.
pub(crate) fn load_all(certs_dir: &Path, provider: &CryptoProvider) -> Vec<(String, IssuedCert)> {
    let entries = match fs::read_dir(certs_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            warn!("cannot read certificate directory {}: {e}", certs_dir.display());
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                warn!("cannot read an entry of {}: {e}", certs_dir.display());
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name == ACCOUNTS_DIR || name.starts_with('.') {
            continue;
        }
        match ferry_core::validate::domain(&name) {
            Ok(normalized) if normalized == name => {}
            _ => {
                debug!("ignoring {}: not a normalized host name", path.display());
                continue;
            }
        }
        match load_host(&path, &name, provider) {
            Ok(cert) => out.push((name, cert)),
            Err(e) => warn!("skipping certificate in {}: {e}", path.display()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::tests::provider;
    use chrono::Duration;

    fn pem(name: &str, days_left: i64) -> (String, String) {
        let now = Utc::now();
        certs::self_signed_pem(name, now - Duration::days(1), now + Duration::days(days_left)).unwrap()
    }

    #[test]
    fn account_paths_are_separate_per_directory() {
        let dir = Path::new("/c");
        let prod = account_path(dir, crate::LETS_ENCRYPT_PRODUCTION);
        let staging = account_path(dir, crate::LETS_ENCRYPT_STAGING);
        assert_ne!(prod, staging);
        assert_eq!(prod, Path::new("/c/accounts/acme-v02.api.letsencrypt.org_directory.json"));
        assert_eq!(account_path(dir, "https://localhost:14000/dir"), Path::new("/c/accounts/localhost_14000_dir.json"));
        assert_eq!(account_path(dir, "../../etc/passwd").parent(), Some(Path::new("/c/accounts")));
        assert_eq!(account_path(dir, ""), Path::new("/c/accounts/default.json"));
    }

    #[test]
    fn save_and_load_round_trip_with_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let (cert, key) = pem("app.example.com", 90);
        let now = Utc::now();
        let meta = CertMeta {
            issued_at: now,
            not_after: now + Duration::days(90),
            directory_url: Some(crate::LETS_ENCRYPT_STAGING.into()),
        };
        save_certificate(tmp.path(), "app.example.com", &cert, &key, &meta).unwrap();
        // overwriting works too
        save_certificate(tmp.path(), "app.example.com", &cert, &key, &meta).unwrap();
        let dir = tmp.path().join("app.example.com");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let stored: CertMeta = serde_json::from_slice(&fs::read(dir.join(META_FILE)).unwrap()).unwrap();
        assert_eq!(stored.issued_at, now);
        // legacy meta.json without directory_url still parses
        let legacy: CertMeta =
            serde_json::from_str(r#"{"issued_at":"2026-01-01T00:00:00Z","not_after":"2026-04-01T00:00:00Z"}"#).unwrap();
        assert!(legacy.directory_url.is_none());
        // no temp files left behind
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);

        let loaded = load_all(tmp.path(), &provider());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, "app.example.com");
        assert_eq!(loaded[0].1.issued_at, now);
        assert_eq!(loaded[0].1.directory_url.as_deref(), Some(crate::LETS_ENCRYPT_STAGING));
        assert_eq!(loaded[0].1.not_after.date_naive(), (now + Duration::days(90)).date_naive());
    }

    #[test]
    fn corrupt_entries_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let p = provider();
        let now = Utc::now();
        let meta = CertMeta { issued_at: now, not_after: now, directory_url: None };
        let (good_cert, good_key) = pem("good.example.com", 90);
        save_certificate(tmp.path(), "good.example.com", &good_cert, &good_key, &meta).unwrap();
        // garbage certificate
        save_certificate(tmp.path(), "garbage.example.com", "not a cert", &good_key, &meta).unwrap();
        // key of another certificate
        let (_, other_key) = pem("x.example.com", 90);
        save_certificate(tmp.path(), "mismatch.example.com", &good_cert, &other_key, &meta).unwrap();
        // missing key
        let missing = tmp.path().join("nokey.example.com");
        fs::create_dir_all(&missing).unwrap();
        fs::write(missing.join(CERT_FILE), &good_cert).unwrap();
        // certificate of another host
        save_certificate(tmp.path(), "wrong.example.com", &good_cert, &good_key, &meta).unwrap();
        // corrupt meta.json only: still loaded, falls back to the certificate
        let (bm_cert, bm_key) = pem("badmeta.example.com", 90);
        save_certificate(tmp.path(), "badmeta.example.com", &bm_cert, &bm_key, &meta).unwrap();
        fs::write(tmp.path().join("badmeta.example.com").join(META_FILE), "{").unwrap();
        // not host directories
        fs::create_dir_all(tmp.path().join(ACCOUNTS_DIR)).unwrap();
        fs::create_dir_all(tmp.path().join("Not A Host")).unwrap();
        fs::write(tmp.path().join("stray.txt"), "x").unwrap();

        let mut names: Vec<String> = load_all(tmp.path(), &p).into_iter().map(|(n, _)| n).collect();
        names.sort();
        assert_eq!(names, vec!["badmeta.example.com", "good.example.com"]);
        assert!(load_all(&tmp.path().join("missing"), &p).is_empty());
    }
}
