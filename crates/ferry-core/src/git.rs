//! Git URL helpers shared by the builder (logging) and the API (webhooks).

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
        s = stripped.to_string();
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
        assert_eq!(normalize_repo_url("/tmp/app/.git"), "/tmp/app/");
        assert_eq!(normalize_repo_url("/tmp/app/"), "/tmp/app");
        assert_eq!(normalize_repo_url("file:///tmp/app"), "/tmp/app");
    }

    #[test]
    fn redacts() {
        assert_eq!(redact_url("https://u:t@github.com/a/b"), "https://***@github.com/a/b");
        assert_eq!(redact_url("https://github.com/a/b"), "https://github.com/a/b");
        assert_eq!(redact_url("git@github.com:a/b"), "git@github.com:a/b");
    }
}
