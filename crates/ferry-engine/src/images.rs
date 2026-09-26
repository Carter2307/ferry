//! Image reference helpers, the retention policy for built images, and why
//! an image a deploy needs is missing.

use std::collections::HashSet;
use std::fmt;

use ferry_core::{Deploy, Result, Service};
use tracing::debug;

use crate::spec;
use crate::state::Inner;

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

/// Why an image a deploy used is no longer in Docker (for error messages:
/// the way out differs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MissingImage {
    /// Deleted by Ferry's retention: older than the newest `keep` images
    /// built for the service.
    Retention { keep: usize },
    /// Removed from Docker by something else (`docker rmi`, `docker image
    /// prune`, a reset Docker...).
    RemovedFromDocker,
}

impl fmt::Display for MissingImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissingImage::Retention { keep } => write!(
                f,
                "it was cleaned up by image retention, which keeps only the newest {keep} images built for a service"
            ),
            MissingImage::RemovedFromDocker => {
                f.write_str("it was removed from Docker outside Ferry, e.g. by `docker rmi` or `docker image prune`")
            }
        }
    }
}

/// Retention only ever deletes the service's own images beyond the newest
/// `keep`: a missing image in that range was cleaned up by Ferry, anything
/// else was removed behind its back. `deploys` is newest first.
pub(crate) fn classify_missing(deploys: &[Deploy], repo: &str, keep: usize, image: &str) -> MissingImage {
    if images_to_remove(deploys, repo, keep, &HashSet::new()).iter().any(|i| i == image) {
        MissingImage::Retention { keep: keep.max(1) }
    } else {
        MissingImage::RemovedFromDocker
    }
}

/// Why `image` (used by a deploy of `svc`) is gone. The live deploy's image
/// is never deleted by retention, so for it the answer is always "removed
/// from Docker".
pub(crate) async fn missing_reason(inner: &Inner, svc: &Service, image: &str, live: bool) -> MissingImage {
    if live {
        return MissingImage::RemovedFromDocker;
    }
    match inner.store.deploys_with_images(&svc.id).await {
        Ok(deploys) => classify_missing(&deploys, &inner.naming.image_repo(&svc.name), inner.config.keep_images, image),
        Err(e) => {
            debug!(service = %svc.name, "cannot tell why image {image} is missing: {e}");
            MissingImage::RemovedFromDocker
        }
    }
}

/// The live deploy of `svc` and the image its instances run, when that
/// image is no longer in Docker (`None`: no live deploy, or its image exists).
pub(crate) async fn missing_live_image(inner: &Inner, svc: &Service) -> Result<Option<(Deploy, String)>> {
    let Some(live_id) = svc.live_deploy_id.as_deref() else {
        return Ok(None);
    };
    let Some(live) = inner.store.get_deploy(live_id).await? else {
        return Ok(None);
    };
    let image = match spec::load(&inner.store, &live.id).await? {
        Some(spec) => spec.image,
        None => match live.image.clone() {
            Some(image) => image,
            // Live without an image (should not happen): nothing can start.
            None => return Ok(Some((live, String::new()))),
        },
    };
    if inner.docker.image_exists(&image).await? { Ok(None) } else { Ok(Some((live, image))) }
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

    #[test]
    fn missing_images_are_explained() {
        let deploys = vec![dep(Some("ferry/web:d3")), dep(Some("ferry/web:d2")), dep(Some("ferry/web:d1"))];
        // Beyond the newest 2 own images: retention deleted it.
        assert_eq!(classify_missing(&deploys, "ferry/web", 2, "ferry/web:d1"), MissingImage::Retention { keep: 2 });
        // Within the retention window, or not one of Ferry's images: removed outside Ferry.
        assert_eq!(classify_missing(&deploys, "ferry/web", 2, "ferry/web:d2"), MissingImage::RemovedFromDocker);
        assert_eq!(classify_missing(&deploys, "ferry/web", 2, "nginx:alpine"), MissingImage::RemovedFromDocker);
        assert!(MissingImage::Retention { keep: 2 }.to_string().contains("image retention"));
        assert!(MissingImage::RemovedFromDocker.to_string().contains("removed from Docker"));
    }
}
