//! Git URL helpers shared by the builder (logging), the API (webhooks) and
//! git connections (which repositories a connected account's token is for).

/// Canonical form used to match webhook payloads against services:
/// lowercase `host/owner/repo` without scheme, credentials, port or `.git`.
///
/// `https://user:tok@GitHub.com/Owner/Repo.git` → `github.com/owner/repo`
/// `git@github.com:owner/repo.git`             → `github.com/owner/repo`
/// `ssh://git@github.com:22/owner/repo`        → `github.com/owner/repo`
/// Local paths are returned trimmed, without a trailing `/` or `.git`.
pub fn normalize_repo_url(url: &str) -> String {
    let mut s = url.trim().trim_end_matches('/').to_string();
    if let Some(stripped) = s.strip_suffix(".git") {
        s = stripped.trim_end_matches('/').to_string();
    }
    let is_local = s.starts_with('/') || s.starts_with("file://") || s.starts_with('.');
    if is_local {
        return s.trim_start_matches("file://").to_string();
    }
    // strip scheme
    if let Some(idx) = s.find("://") {
        s = s[idx + 3..].to_string();
    }
    // strip credentials
    if let Some(at) = s.find('@')
        && s[..at].find('/').is_none()
    {
        s = s[at + 1..].to_string();
    }
    // scp-like syntax host:owner/repo, or host:port/owner/repo
    if let Some(colon) = s.find(':') {
        let (host, rest) = s.split_at(colon);
        let rest = &rest[1..];
        let rest = match rest.split_once('/') {
            Some((maybe_port, tail)) if maybe_port.chars().all(|c| c.is_ascii_digit()) => tail.to_string(),
            _ => rest.to_string(),
        };
        s = format!("{host}/{rest}");
    }
    s.to_ascii_lowercase()
}

/// Remove credentials from a URL for display in logs:
/// `https://user:token@github.com/a/b` → `https://***@github.com/a/b`.
pub fn redact_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at) = rest.find('@')
            && !rest[..at].contains('/')
        {
            return format!("{}***@{}", &url[..scheme_end + 3], &rest[at + 1..]);
        }
    }
    url.to_string()
}

/// An http(s) URL, split into what decides where a request goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpUrl {
    /// `http` or `https`.
    pub scheme: &'static str,
    /// Lowercase host name, or an IPv6 address in brackets.
    pub host: String,
    /// The port of the URL, else the scheme's default (80 / 443).
    pub port: u16,
    /// The path with its leading `/` (empty when the URL has none), without
    /// query string or fragment.
    pub path: String,
}

impl HttpUrl {
    fn default_port(&self) -> u16 {
        if self.scheme == "https" { 443 } else { 80 }
    }

    /// `host[:port]`, the port only when it isn't the scheme's default.
    pub fn authority(&self) -> String {
        if self.port == self.default_port() { self.host.clone() } else { format!("{}:{}", self.host, self.port) }
    }

    /// `scheme://host[:port]`.
    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme, self.authority())
    }
}

/// Parse an http(s) URL that only says where to go: no credentials, a host
/// of letters, digits, `.` and `-` (or a bracketed IPv6 address) and an
/// optional numeric port. `None` for anything else (other schemes, local
/// paths, userinfo, unusual characters), so that a token is never matched
/// against a URL another parser (git's, curl's) could read differently.
pub fn parse_http_url(url: &str) -> Option<HttpUrl> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" => "http",
        "https" => "https",
        _ => return None,
    };
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (addr, after) = v6.split_once(']')?;
            if addr.is_empty() || !addr.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') {
                return None;
            }
            let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
            (format!("[{}]", addr.to_ascii_lowercase()), port)
        }
        None => {
            let (host, port) = match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            };
            let plain = !host.is_empty()
                && !host.starts_with(['.', '-'])
                && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
            if !plain {
                return None;
            }
            (host.to_ascii_lowercase(), port)
        }
    };
    let port = match port {
        None if scheme == "https" => 443,
        None => 80,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => {
            p.parse::<u16>().ok().filter(|p| *p != 0)?
        }
        Some(_) => return None,
    };
    let path = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
    Some(HttpUrl { scheme, host, port, path: path.to_string() })
}

/// Both are http(s) URLs (see [`parse_http_url`]) of the same scheme, host
/// and port.
pub fn same_http_origin(a: &str, b: &str) -> bool {
    match (parse_http_url(a), parse_http_url(b)) {
        (Some(a), Some(b)) => (a.scheme, a.host, a.port) == (b.scheme, b.host, b.port),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        for u in [
            "https://github.com/Owner/Repo.git",
            "https://user:tok@github.com/owner/repo",
            "git@github.com:owner/repo.git",
            "ssh://git@github.com:22/owner/repo",
            "http://github.com/owner/repo/",
        ] {
            assert_eq!(normalize_repo_url(u), "github.com/owner/repo", "{u}");
        }
        assert_eq!(normalize_repo_url("/tmp/app/.git"), "/tmp/app");
        assert_eq!(normalize_repo_url("/tmp/app/"), "/tmp/app");
        assert_eq!(normalize_repo_url("file:///tmp/app"), "/tmp/app");
    }

    #[test]
    fn redacts() {
        assert_eq!(redact_url("https://u:t@github.com/a/b"), "https://***@github.com/a/b");
        assert_eq!(redact_url("https://github.com/a/b"), "https://github.com/a/b");
        assert_eq!(redact_url("git@github.com:a/b"), "git@github.com:a/b");
    }

    #[test]
    fn parses_plain_http_urls() {
        let u = parse_http_url("HTTPS://GitHub.com/Owner/Repo.git?x=1#frag").unwrap();
        assert_eq!(
            (u.scheme, u.host.as_str(), u.port, u.path.as_str()),
            ("https", "github.com", 443, "/Owner/Repo.git")
        );
        assert_eq!((u.authority(), u.origin()), ("github.com".to_string(), "https://github.com".to_string()));
        // The default port may be written out; another one is part of the origin.
        assert_eq!(parse_http_url("https://github.com:443/a/b").unwrap().origin(), "https://github.com");
        let local = parse_http_url("http://127.0.0.1:8929").unwrap();
        assert_eq!((local.origin().as_str(), local.path.as_str()), ("http://127.0.0.1:8929", ""));
        assert_eq!(parse_http_url("http://[::1]:3000/g/p").unwrap().authority(), "[::1]:3000");
        assert_eq!(parse_http_url("https://[2001:DB8::1]/x").unwrap().host, "[2001:db8::1]");
        // Anything that isn't a plain http(s) URL, or that carries credentials.
        for bad in [
            "",
            "github.com/a/b",
            "git@github.com:a/b.git",
            "ssh://git@github.com/a/b",
            "file:///srv/repo",
            "/srv/repo",
            "https://",
            "https:///a/b",
            "https://user@github.com/a/b",
            "https://user:tok@github.com/a/b",
            "https://github.com@evil.example/a/b",
            "https://evil.example\\@github.com/a/b",
            "https://github.com:/a/b",
            "https://github.com:0/a/b",
            "https://github.com:99999/a/b",
            "https://github.com:80a/a/b",
            "https://git hub.com/a/b",
            "https://.github.com/a",
            "https://[::1/a",
            "https://[::1]x/a",
            "https://[]/a",
        ] {
            assert_eq!(parse_http_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn origins() {
        assert!(same_http_origin("https://github.com/a/b.git", "https://GITHUB.com:443"));
        assert!(same_http_origin("http://127.0.0.1:8080/x", "http://127.0.0.1:8080"));
        assert!(!same_http_origin("https://github.com/a/b", "http://github.com/a/b"));
        assert!(!same_http_origin("https://github.com/a/b", "https://gitlab.com/a/b"));
        assert!(!same_http_origin("https://github.com/a/b", "https://github.com:8443/a/b"));
        assert!(!same_http_origin("https://github.com/a/b", "https://api.github.com/a/b"));
        assert!(!same_http_origin("git@github.com:a/b", "git@github.com:a/b"));
        assert!(!same_http_origin("/srv/repo", "/srv/repo"));
    }
}
