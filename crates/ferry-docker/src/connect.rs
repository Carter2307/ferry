//! Locating the Docker daemon and agreeing on an API version.

use std::fmt;

use bollard::errors::Error as BollardError;
use bollard::{API_DEFAULT_VERSION, ClientVersion};
use ferry_core::{Error, Result};

/// Request timeout (seconds) for ordinary API calls. Calls that legitimately
/// block (wait, followed logs, exec, stop) use longer, per-call timeouts.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[cfg(unix)]
const DEFAULT_LOCAL_ADDR: &str = "unix:///var/run/docker.sock";
#[cfg(windows)]
const DEFAULT_LOCAL_ADDR: &str = "npipe:////./pipe/docker_engine";

/// A place the daemon may listen on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// Unix socket (`unix:///path`) or Windows named pipe.
    Local(String),
    /// Plain TCP (`tcp://host:port`, `http://host:port`).
    Http(String),
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Local(addr) | Endpoint::Http(addr) => f.write_str(addr),
        }
    }
}

impl Endpoint {
    fn client(&self, version: &ClientVersion) -> Result<bollard::Docker, BollardError> {
        match self {
            Endpoint::Local(addr) => bollard::Docker::connect_with_local(addr, DEFAULT_TIMEOUT_SECS, version),
            Endpoint::Http(addr) => bollard::Docker::connect_with_http(addr, DEFAULT_TIMEOUT_SECS, version),
        }
    }
}

/// Endpoint for a `DOCKER_HOST` value.
fn endpoint_for_host(host: &str) -> Result<Endpoint> {
    if host.starts_with("unix://") || host.starts_with("npipe://") {
        Ok(Endpoint::Local(host.to_string()))
    } else if host.starts_with("tcp://") || host.starts_with("http://") {
        Ok(Endpoint::Http(host.to_string()))
    } else {
        Err(Error::Docker(format!("unsupported DOCKER_HOST '{host}' (use unix://, tcp:// or http://)")))
    }
}

/// Candidate endpoints in order: `DOCKER_HOST` (or the platform default
/// socket), then on macOS the Docker Desktop socket `~/.docker/run/docker.sock`.
pub(crate) fn candidates(docker_host: Option<&str>, home: Option<&str>) -> Result<Vec<Endpoint>> {
    let mut out = Vec::new();
    match docker_host.map(str::trim).filter(|h| !h.is_empty()) {
        Some(host) => out.push(endpoint_for_host(host)?),
        None => out.push(Endpoint::Local(DEFAULT_LOCAL_ADDR.to_string())),
    }
    if cfg!(target_os = "macos")
        && let Some(home) = home.map(str::trim).filter(|h| !h.is_empty())
    {
        let desktop = Endpoint::Local(format!("unix://{}/.docker/run/docker.sock", home.trim_end_matches('/')));
        if !out.contains(&desktop) {
            out.push(desktop);
        }
    }
    Ok(out)
}

/// The API version named in a daemon's version-mismatch error:
/// "client version 1.53 is too new. Maximum supported API version is 1.47" or
/// "client version 1.24 is too old. Minimum supported API version is 1.44, …".
pub(crate) fn supported_version_from_error(message: &str) -> Option<ClientVersion> {
    let lower = message.to_ascii_lowercase();
    let start = lower.find("supported api version is")? + "supported api version is".len();
    let version: String =
        message[start..].trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let (major, minor) = version.trim_end_matches('.').split_once('.')?;
    Some(ClientVersion { major_version: major.parse().ok()?, minor_version: minor.parse().ok()? })
}

/// Connect to one endpoint: ping with bollard's default API version, retry
/// with the version the daemon asks for if it rejects ours, then negotiate
/// (downgrade to the daemon's maximum when it is older than the client).
pub(crate) async fn connect_endpoint(endpoint: &Endpoint) -> Result<bollard::Docker, String> {
    let client = endpoint.client(API_DEFAULT_VERSION).map_err(|e| crate::errors::detail(&e))?;
    let client = match client.ping().await {
        Ok(_) => client,
        Err(err) => {
            let wanted = match &err {
                BollardError::DockerResponseServerError { status_code: 400, message } => {
                    supported_version_from_error(message)
                }
                _ => None,
            };
            let Some(version) = wanted.filter(|v| v != API_DEFAULT_VERSION) else {
                return Err(crate::errors::detail(&err));
            };
            if version > *API_DEFAULT_VERSION {
                tracing::warn!(%version, client = %API_DEFAULT_VERSION, "Docker daemon requires a newer API version than this client was built for");
            }
            let retry = endpoint.client(&version).map_err(|e| crate::errors::detail(&e))?;
            retry.ping().await.map_err(|e| crate::errors::detail(&e))?;
            retry
        }
    };
    client.negotiate_version().await.map_err(|e| format!("negotiating API version: {}", crate::errors::detail(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_errors() {
        let v = supported_version_from_error("client version 1.53 is too new. Maximum supported API version is 1.47")
            .unwrap();
        assert_eq!((v.major_version, v.minor_version), (1, 47));
        let v = supported_version_from_error(
            "client version 1.24 is too old. Minimum supported API version is 1.44, please upgrade your client",
        )
        .unwrap();
        assert_eq!((v.major_version, v.minor_version), (1, 44));
        assert!(supported_version_from_error("page not found").is_none());
        assert!(supported_version_from_error("Maximum supported API version is x").is_none());
    }

    #[test]
    fn candidate_endpoints() {
        let c = candidates(None, Some("/Users/me")).unwrap();
        assert_eq!(c[0], Endpoint::Local(DEFAULT_LOCAL_ADDR.to_string()));
        if cfg!(target_os = "macos") {
            assert_eq!(c, vec![c[0].clone(), Endpoint::Local("unix:///Users/me/.docker/run/docker.sock".into())]);
        } else {
            assert_eq!(c.len(), 1);
        }
        let c = candidates(Some("tcp://10.0.0.1:2375"), None).unwrap();
        assert_eq!(c, vec![Endpoint::Http("tcp://10.0.0.1:2375".into())]);
        let c = candidates(Some("unix:///Users/me/.docker/run/docker.sock"), Some("/Users/me/")).unwrap();
        assert_eq!(c.len(), 1, "no duplicate endpoint");
        let c = candidates(Some("  "), None).unwrap();
        assert_eq!(c[0], Endpoint::Local(DEFAULT_LOCAL_ADDR.to_string()));
        assert!(candidates(Some("ssh://me@host"), None).is_err());
    }
}
