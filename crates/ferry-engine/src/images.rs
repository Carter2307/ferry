//! Image reference helpers and the retention policy for built images.

use std::collections::HashSet;

use ferry_core::Deploy;

/// The tag of an image reference: `Some("alpine")` for `nginx:alpine`,
/// `None` for `nginx` or `localhost:5000/app`, and `Some("@digest")` for a
/// reference pinned by digest.
pub(crate) fn image_tag(image: &str) -> Option<&str> {
    let image = image.trim();
    if let Some(at) = image.find('@') {
        return Some(&image[at..]);
    }
    let last = image.rsplit('/').next().unwrap_or(image);
    last.split_once(':').map(|(_, tag)| tag).filter(|t| !t.is_empty())
}

/// Image sources are pulled on every deploy when untagged or `latest`
/// (the tag may have moved); pinned tags and digests only when missing.
pub(crate) fn always_pull(image: &str) -> bool {
    match image_tag(image) {
        None => true,
        Some(tag) => tag == "latest",
    }
}

/// True for images this server built for the service (`<prefix>/<name>:<tag>`).
pub(crate) fn is_own_image(image: &str, repo: &str) -> bool {
    image.strip_prefix(repo).is_some_and(|rest| rest.starts_with(':') && rest.len() > 1)
}

/// Images to delete after a deploy: of the service's own images (repository
/// `repo`, never pulled public images), keep the newest `keep` distinct ones
/// plus every `protected` one (the live image, images of active deploys).
/// `deploys` is newest first (as `Store::deploys_with_images` returns them).
pub(crate) fn images_to_remove(
    deploys: &[Deploy],
    repo: &str,
    keep: usize,
    protected: &HashSet<String>,
) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    for d in deploys {
        if let Some(image) = d.image.as_deref()
            && is_own_image(image, repo)
            && !seen.contains(&image)
        {
            seen.push(image);
        }
    }
    seen.into_iter().skip(keep.max(1)).filter(|img| !protected.contains(*img)).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{DeploySource, DeployTrigger};

    fn dep(image: Option<&str>) -> Deploy {
        let mut d = Deploy::new("srv-1", DeployTrigger::Manual, DeploySource::Image { image: "x".into() });
        d.image = image.map(str::to_string);
        d
    }

    #[test]
    fn tags() {
        assert_eq!(image_tag("nginx"), None);
        assert_eq!(image_tag("nginx:alpine"), Some("alpine"));
        assert_eq!(image_tag("nginx:latest"), Some("latest"));
        assert_eq!(image_tag("localhost:5000/app"), None);
        assert_eq!(image_tag("localhost:5000/app:v1"), Some("v1"));
        assert_eq!(image_tag("busybox@sha256:abc"), Some("@sha256:abc"));
        assert!(always_pull("nginx"));
        assert!(always_pull("nginx:latest"));
        assert!(always_pull("registry.example.com:443/team/app"));
        assert!(!always_pull("nginx:alpine"));
        assert!(!always_pull("app@sha256:abc"));
    }

    #[test]
    fn own_images() {
        assert!(is_own_image("ferry/web:dep-1", "ferry/web"));
        assert!(!is_own_image("ferry/web-2:dep-1", "ferry/web"));
        assert!(!is_own_image("ferry/web", "ferry/web"));
        assert!(!is_own_image("ferry/web:", "ferry/web"));
        assert!(!is_own_image("nginx:alpine", "ferry/web"));
    }

    #[test]
    fn retention_keeps_newest_distinct_own_images_and_protected_ones() {
        let deploys = vec![
            dep(Some("ferry/web:d6")),
            dep(Some("ferry/web:d5")),
            dep(Some("nginx:alpine")), // pulled public image: never deleted
            dep(Some("ferry/web:d5")), // reuse (restart) of d5: counted once
            dep(Some("ferry/web:d4")),
            dep(Some("ferry/web:d3")),
            dep(None),
            dep(Some("ferry/web:d2")),
            dep(Some("ferry/web:d1")),
        ];
        let none = HashSet::new();
        assert_eq!(
            images_to_remove(&deploys, "ferry/web", 3, &none),
            vec!["ferry/web:d3", "ferry/web:d2", "ferry/web:d1"]
        );
        let protected: HashSet<String> = ["ferry/web:d2".to_string()].into();
        assert_eq!(images_to_remove(&deploys, "ferry/web", 3, &protected), vec!["ferry/web:d3", "ferry/web:d1"]);
        assert!(images_to_remove(&deploys, "ferry/web", 10, &none).is_empty());
        // keep = 0 still keeps the newest one.
        assert_eq!(images_to_remove(&deploys, "ferry/web", 0, &none).len(), 5);
        assert!(images_to_remove(&deploys, "ferry/api", 1, &none).is_empty());
    }
}
