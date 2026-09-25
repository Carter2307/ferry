//! Image reference parsing for the pull API (`fromImage` + `tag`).

use ferry_core::{Error, Result};

/// Split an image reference into the `fromImage` and `tag` query parameters
/// of `POST /images/create`.
///
/// * `nginx` → (`nginx`, `latest`); `nginx:alpine` → (`nginx`, `alpine`)
/// * registry ports are not tags: `localhost:5000/app` → (`localhost:5000/app`, `latest`)
/// * digests win over tags: `nginx:1.27@sha256:…` → (`nginx`, `sha256:…`) — the
///   daemon accepts a digest in `tag`.
///
/// A tag is always returned: an empty tag would make the daemon pull *every*
/// tag of the repository.
pub(crate) fn split_image_ref(image: &str) -> Result<(String, String)> {
    let image = image.trim();
    let invalid = || Error::invalid(format!("invalid image reference '{image}'"));
    if image.is_empty() || image.chars().any(char::is_whitespace) {
        return Err(invalid());
    }
    if let Some((name, digest)) = image.split_once('@') {
        let (repo, _tag) = split_tag(name);
        if repo.is_empty() || digest.is_empty() || !digest.contains(':') || digest.contains('@') {
            return Err(invalid());
        }
        return Ok((repo.to_string(), digest.to_string()));
    }
    let (repo, tag) = split_tag(image);
    match tag {
        _ if repo.is_empty() => Err(invalid()),
        Some("") => Err(invalid()),
        Some(tag) => Ok((repo.to_string(), tag.to_string())),
        None => Ok((repo.to_string(), "latest".to_string())),
    }
}

/// `repo[:tag]` → (repo, tag). Only a `:` after the last `/` starts a tag, so
/// `host:port/repo` has no tag.
fn split_tag(name: &str) -> (&str, Option<&str>) {
    let last_segment = name.rfind('/').map_or(0, |i| i + 1);
    match name[last_segment..].rfind(':') {
        Some(i) => {
            let idx = last_segment + i;
            (&name[..idx], Some(&name[idx + 1..]))
        }
        None => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> (String, String) {
        split_image_ref(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    #[test]
    fn plain_and_tagged() {
        assert_eq!(split("nginx"), pair("nginx", "latest"));
        assert_eq!(split("nginx:alpine"), pair("nginx", "alpine"));
        assert_eq!(split(" busybox:stable "), pair("busybox", "stable"));
        assert_eq!(split("library/redis:7-alpine"), pair("library/redis", "7-alpine"));
        assert_eq!(split("ferry/web:dep-0123"), pair("ferry/web", "dep-0123"));
        assert_eq!(split("ghcr.io/org/app:v1.2.3"), pair("ghcr.io/org/app", "v1.2.3"));
    }

    #[test]
    fn registry_ports() {
        assert_eq!(split("localhost:5000/app"), pair("localhost:5000/app", "latest"));
        assert_eq!(split("localhost:5000/app:1.0"), pair("localhost:5000/app", "1.0"));
        assert_eq!(split("registry.example.com:443/team/app:edge"), pair("registry.example.com:443/team/app", "edge"));
    }

    #[test]
    fn digests() {
        let d = "sha256:73aaf090f3d85aa34ee199857f03fa3a95c8ede2ffd4cc2cdb5b94e566b11662";
        assert_eq!(split(&format!("busybox@{d}")), pair("busybox", d));
        assert_eq!(split(&format!("nginx:1.27@{d}")), pair("nginx", d));
        assert_eq!(split(&format!("localhost:5000/app@{d}")), pair("localhost:5000/app", d));
        assert_eq!(split(&format!("localhost:5000/app:v2@{d}")), pair("localhost:5000/app", d));
    }

    #[test]
    fn invalid() {
        for bad in ["", "   ", "nginx:", "@sha256:abc", "nginx@", "nginx@abc", ":tag", "a b", "x@sha256:a@b"] {
            assert!(split_image_ref(bad).is_err(), "{bad:?} should be invalid");
        }
    }
}
