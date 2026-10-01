//! CLI configuration: `~/.config/ferry/config.json` (`$XDG_CONFIG_HOME` is
//! honored), overridden by the `FERRY_SERVER` / `FERRY_TOKEN` environment
//! variables, themselves overridden by the `--server` / `--token` flags.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};

/// Server used when nothing else is configured (ferryd's default API address).
pub const DEFAULT_SERVER: &str = "http://127.0.0.1:7878";
pub const ENV_SERVER: &str = "FERRY_SERVER";
pub const ENV_TOKEN: &str = "FERRY_TOKEN";

/// Contents of `config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Values given on the command line (highest precedence).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub server: Option<String>,
    pub token: Option<String>,
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    File,
    Default,
}

/// Effective connection settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Normalized server URL (no trailing slash).
    pub server: String,
    pub server_source: Source,
    pub token: Option<String>,
    pub token_source: Option<Source>,
    /// Server of the saved login when its token was *not* used because the
    /// effective server is a different one (the token belongs to that server).
    pub saved_login_server: Option<String>,
}

impl Settings {
    /// The API token, or the "log in first" error.
    pub fn require_token(&self) -> Result<&str> {
        self.token.as_deref().ok_or_else(|| match &self.saved_login_server {
            Some(saved) => anyhow!(
                "no token for {}: the saved login is for {saved}, and its token is only sent there; \
                 pass --token or set {ENV_TOKEN} (or run 'ferry login --server {}')",
                self.server,
                self.server
            ),
            None => {
                anyhow!("not logged in: run 'ferry login --server <URL>' first (or set {ENV_SERVER} and {ENV_TOKEN})")
            }
        })
    }
}

/// Path of the config file: `$XDG_CONFIG_HOME/ferry/config.json` when
/// `XDG_CONFIG_HOME` is an absolute path, else `~/.config/ferry/config.json`.
pub fn config_path() -> Option<PathBuf> {
    config_path_from(std::env::var_os("XDG_CONFIG_HOME"), std::env::home_dir())
}

pub fn config_path_from(xdg_config_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let xdg = xdg_config_home.filter(|x| !x.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute());
    match xdg {
        Some(x) => Some(x.join("ferry").join("config.json")),
        None => home.filter(|h| !h.as_os_str().is_empty()).map(|h| h.join(".config").join("ferry").join("config.json")),
    }
}

/// Read the config file; `Ok(None)` when it doesn't exist.
pub fn load(path: &Path) -> Result<Option<FileConfig>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading config file {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Some(FileConfig::default()));
    }
    let cfg = serde_json::from_str(&text).with_context(|| format!("invalid config file {}", path.display()))?;
    Ok(Some(cfg))
}

/// Write the config file atomically with mode 0600 (directory 0700).
pub fn save(path: &Path, cfg: &FileConfig) -> Result<()> {
    let dir = path.parent().ok_or_else(|| anyhow!("invalid config path {}", path.display()))?;
    create_private_dir(dir).with_context(|| format!("creating config directory {}", dir.display()))?;
    let mut json = serde_json::to_string_pretty(cfg).context("serializing config")?;
    json.push('\n');

    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config.json");
    let tmp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let _ = std::fs::remove_file(&tmp);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        set_private_file(&tmp)?;
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing config file {}", path.display()));
    }
    Ok(())
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

fn set_private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Combine flags > environment > config file > default.
/// `env` looks up an environment variable (injectable for tests).
///
/// The saved token is only used for the server it was saved with: when
/// `--server` / `FERRY_SERVER` points somewhere else, a token must come from
/// `--token` / `FERRY_TOKEN`, so a credential never leaks to another host.
pub fn resolve(flags: &Overrides, env: impl Fn(&str) -> Option<String>, file: Option<&FileConfig>) -> Result<Settings> {
    let pick = |flag: &Option<String>, key: &str, file_val: Option<&String>| -> Option<(String, Source)> {
        non_empty(flag.clone())
            .map(|v| (v, Source::Flag))
            .or_else(|| non_empty(env(key)).map(|v| (v, Source::Env)))
            .or_else(|| non_empty(file_val.cloned()).map(|v| (v, Source::File)))
    };
    let (server, server_source) = pick(&flags.server, ENV_SERVER, file.and_then(|f| f.server.as_ref()))
        .unwrap_or_else(|| (DEFAULT_SERVER.to_string(), Source::Default));
    let server = normalize_server(&server).with_context(|| match server_source {
        Source::Flag => "invalid --server".to_string(),
        Source::Env => format!("invalid {ENV_SERVER}"),
        Source::File => "invalid server in config file".to_string(),
        Source::Default => "invalid default server".to_string(),
    })?;

    // The server the saved token belongs to (a file without a server was
    // written for the default one).
    let saved_server = file.map(|f| {
        let raw = non_empty(f.server.clone()).unwrap_or_else(|| DEFAULT_SERVER.to_string());
        normalize_server(&raw).unwrap_or(raw)
    });
    let file_token_usable = saved_server.as_deref() == Some(server.as_str());
    let file_token = file.and_then(|f| f.token.as_ref()).filter(|_| file_token_usable);
    let token = pick(&flags.token, ENV_TOKEN, file_token);
    let saved_login_server = match (&token, file) {
        (None, Some(f)) if non_empty(f.token.clone()).is_some() && !file_token_usable => saved_server,
        _ => None,
    };
    Ok(Settings {
        server,
        server_source,
        token_source: token.as_ref().map(|(_, s)| *s),
        token: token.map(|(t, _)| t),
        saved_login_server,
    })
}

/// Normalize a server URL: add `http://` when no scheme is given, require
/// http(s) with a host, drop query/fragment and the trailing slash.
pub fn normalize_server(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("server URL is empty");
    }
    let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("http://{raw}") };
    let mut url = Url::parse(&with_scheme).with_context(|| format!("'{raw}' is not a valid URL"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        bail!("'{raw}': only http:// and https:// servers are supported");
    }
    if url.host_str().is_none_or(str::is_empty) {
        bail!("'{raw}': missing host");
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn path_honors_xdg_config_home() {
        let p = config_path_from(Some("/xdg".into()), Some("/home/u".into())).unwrap();
        assert_eq!(p, PathBuf::from("/xdg/ferry/config.json"));
        let p = config_path_from(None, Some("/home/u".into())).unwrap();
        assert_eq!(p, PathBuf::from("/home/u/.config/ferry/config.json"));
        // Empty or relative XDG_CONFIG_HOME is ignored (XDG spec).
        let p = config_path_from(Some("".into()), Some("/home/u".into())).unwrap();
        assert_eq!(p, PathBuf::from("/home/u/.config/ferry/config.json"));
        let p = config_path_from(Some("rel/dir".into()), Some("/home/u".into())).unwrap();
        assert_eq!(p, PathBuf::from("/home/u/.config/ferry/config.json"));
        assert!(config_path_from(None, None).is_none());
    }

    #[test]
    fn save_and_load_roundtrip_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("ferry").join("config.json");
        assert_eq!(load(&path).unwrap(), None);
        let cfg = FileConfig { server: Some("http://example.com:7878".into()), token: Some("fy_secret".into()) };
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path).unwrap(), Some(cfg.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dmode = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
            assert_eq!(dmode, 0o700);
        }
        // Overwrite keeps working and leaves no temp files behind.
        let cfg2 = FileConfig { server: Some("http://other".into()), token: None };
        save(&path, &cfg2).unwrap();
        assert_eq!(load(&path).unwrap(), Some(cfg2));
        let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn load_rejects_invalid_json_and_accepts_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{not json").unwrap();
        let err = load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("invalid config file"));
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(load(&path).unwrap(), Some(FileConfig::default()));
    }

    #[test]
    fn precedence_flags_then_env_then_file_then_default() {
        let file = FileConfig { server: Some("http://file:1".into()), token: Some("file-token".into()) };
        let flags = Overrides { server: Some("http://flag:1".into()), token: Some("flag-token".into()) };
        let env = env_of(&[(ENV_SERVER, "http://env:1"), (ENV_TOKEN, "env-token")]);

        let s = resolve(&flags, &env, Some(&file)).unwrap();
        assert_eq!((s.server.as_str(), s.token.as_deref()), ("http://flag:1", Some("flag-token")));
        assert_eq!((s.server_source, s.token_source), (Source::Flag, Some(Source::Flag)));

        let s = resolve(&Overrides::default(), &env, Some(&file)).unwrap();
        assert_eq!((s.server.as_str(), s.token.as_deref()), ("http://env:1", Some("env-token")));
        assert_eq!(s.server_source, Source::Env);

        let s = resolve(&Overrides::default(), env_of(&[]), Some(&file)).unwrap();
        assert_eq!((s.server.as_str(), s.token.as_deref()), ("http://file:1", Some("file-token")));
        assert_eq!(s.token_source, Some(Source::File));

        let s = resolve(&Overrides::default(), env_of(&[]), None).unwrap();
        assert_eq!(s.server, DEFAULT_SERVER);
        assert_eq!(s.server_source, Source::Default);
        assert!(s.token.is_none());
        let err = s.require_token().unwrap_err().to_string();
        assert!(err.contains("run 'ferry login"), "{err}");

        // Mixed: token from env, server from file; empty values are ignored.
        let env = env_of(&[(ENV_SERVER, "  "), (ENV_TOKEN, "env-token")]);
        let flags = Overrides { server: Some(String::new()), token: None };
        let s = resolve(&flags, &env, Some(&file)).unwrap();
        assert_eq!((s.server.as_str(), s.token.as_deref()), ("http://file:1", Some("env-token")));
    }

    #[test]
    fn saved_token_is_only_used_for_its_own_server() {
        let file = FileConfig { server: Some("http://127.0.0.1:7878".into()), token: Some("fy_A".into()) };

        // Another server via --server or FERRY_SERVER: the saved token stays home.
        for (flags, env) in [
            (Overrides { server: Some("http://127.0.0.1:9999".into()), token: None }, env_of(&[])),
            (Overrides::default(), env_of(&[(ENV_SERVER, "http://evil.example.com")])),
        ] {
            let s = resolve(&flags, &env, Some(&file)).unwrap();
            assert_eq!(s.token, None, "{s:?}");
            assert_eq!(s.token_source, None);
            assert_eq!(s.saved_login_server.as_deref(), Some("http://127.0.0.1:7878"));
            let err = s.require_token().unwrap_err().to_string();
            assert!(
                err.contains(&format!("no token for {}", s.server))
                    && err.contains("saved login is for http://127.0.0.1:7878"),
                "{err}"
            );
            assert!(err.contains("--token") && err.contains(ENV_TOKEN), "{err}");
        }

        // An explicit token for the other server is used.
        let flags = Overrides { server: Some("http://127.0.0.1:9999".into()), token: None };
        let s = resolve(&flags, env_of(&[(ENV_TOKEN, "fy_B")]), Some(&file)).unwrap();
        assert_eq!((s.token.as_deref(), s.token_source), (Some("fy_B"), Some(Source::Env)));
        assert_eq!(s.saved_login_server, None);

        // The same server, spelled differently but equal once normalized.
        for same in ["http://127.0.0.1:7878/", "127.0.0.1:7878", " http://127.0.0.1:7878/?x=1 "] {
            let flags = Overrides { server: Some(same.into()), token: None };
            let s = resolve(&flags, env_of(&[]), Some(&file)).unwrap();
            assert_eq!((s.token.as_deref(), s.token_source), (Some("fy_A"), Some(Source::File)), "{same}");
        }

        // A config file without a server was saved for the default server.
        let token_only = FileConfig { server: None, token: Some("fy_D".into()) };
        let s = resolve(&Overrides::default(), env_of(&[]), Some(&token_only)).unwrap();
        assert_eq!((s.server.as_str(), s.token.as_deref()), (DEFAULT_SERVER, Some("fy_D")));
        let flags = Overrides { server: Some("http://other:1".into()), token: None };
        let s = resolve(&flags, env_of(&[]), Some(&token_only)).unwrap();
        assert_eq!(s.token, None);
        assert_eq!(s.saved_login_server.as_deref(), Some(DEFAULT_SERVER));

        // No saved token at all: the plain "not logged in" message.
        let server_only = FileConfig { server: Some("http://127.0.0.1:7878".into()), token: None };
        let s = resolve(&flags, env_of(&[]), Some(&server_only)).unwrap();
        assert_eq!(s.saved_login_server, None);
        assert!(s.require_token().unwrap_err().to_string().contains("not logged in"));
    }

    #[test]
    fn invalid_server_is_reported_with_its_source() {
        let flags = Overrides { server: Some("ftp://x".into()), token: None };
        let err = resolve(&flags, env_of(&[]), None).unwrap_err();
        assert!(format!("{err:#}").contains("invalid --server"));
    }

    #[test]
    fn server_normalization() {
        assert_eq!(normalize_server("localhost:7878").unwrap(), "http://localhost:7878");
        assert_eq!(normalize_server("http://127.0.0.1:7878/").unwrap(), "http://127.0.0.1:7878");
        assert_eq!(
            normalize_server(" https://ferry.example.com/base/?x=1#f ").unwrap(),
            "https://ferry.example.com/base"
        );
        assert!(normalize_server("").is_err());
        assert!(normalize_server("ftp://example.com").is_err());
        assert!(normalize_server("http://").is_err());
    }
}
