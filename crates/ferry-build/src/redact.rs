//! Credential redaction for anything that may reach logs or error messages.

use ferry_core::git::redact_url;

/// Remove credentials of `url` (and of any other `scheme://user:pass@host`
/// URL) from free-form text such as git's stderr.
pub(crate) fn redact_text(text: &str, url: &str) -> String {
    let mut out = text.to_string();
    let redacted = redact_url(url);
    if redacted != url {
        out = out.replace(url, &redacted);
        for secret in credential_parts(url) {
            out = out.replace(&secret, "***");
        }
    }
    redact_embedded_urls(&out)
}

/// Pieces of the userinfo of `url` worth masking on their own: the whole
/// userinfo, the password, and a lone username (often a token).
fn credential_parts(url: &str) -> Vec<String> {
    let Some(scheme_end) = url.find("://") else { return Vec::new() };
    let rest = &url[scheme_end + 3..];
    let Some(at) = rest.find('@') else { return Vec::new() };
    let userinfo = &rest[..at];
    if userinfo.contains('/') {
        return Vec::new();
    }
    let mut parts = vec![userinfo.to_string()];
    match userinfo.split_once(':') {
        Some((user, pass)) => {
            parts.push(pass.to_string());
            if user.len() >= 20 {
                parts.push(user.to_string());
            }
        }
        None => parts.push(userinfo.to_string()),
    }
    // Only mask pieces long enough not to garble unrelated text.
    parts.retain(|p| p.len() >= 6);
    parts.sort_by_key(|p| std::cmp::Reverse(p.len()));
    parts.dedup();
    parts
}

/// Mask the userinfo of every `scheme://userinfo@host` occurrence in `text`.
fn redact_embedded_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("://") {
        let (head, tail) = rest.split_at(idx + 3);
        out.push_str(head);
        // The authority ends at the first '/', whitespace or quote.
        let end = tail.find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"').unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) if at > 0 => {
                out.push_str("***@");
                out.push_str(&authority[at + 1..]);
            }
            _ => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_url_and_parts() {
        let url = "https://oauth2:s3cr3t-token@gitlab.example.com/a/b.git";
        let text = format!("fatal: unable to access '{url}': 403\nhint: token s3cr3t-token rejected");
        let r = redact_text(&text, url);
        assert!(!r.contains("s3cr3t-token"), "{r}");
        assert!(r.contains("https://***@gitlab.example.com/a/b.git"), "{r}");
    }

    #[test]
    fn redacts_token_only_userinfo() {
        let url = "https://ghp_abcdefghijklmnop@github.com/a/b";
        let r = redact_text("could not read from ghp_abcdefghijklmnop", url);
        assert_eq!(r, "could not read from ***");
    }

    #[test]
    fn redacts_other_urls_in_text() {
        let r = redact_text("From https://user:pw@host.example/x and ssh://git@github.com/a", "/local/path");
        assert_eq!(r, "From https://***@host.example/x and ssh://***@github.com/a");
        assert_eq!(redact_text("plain text, no urls", "x"), "plain text, no urls");
        assert_eq!(redact_text("see https://github.com/a/b", "x"), "see https://github.com/a/b");
    }
}
