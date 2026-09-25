//! Mapping of bollard errors onto `ferry_core::Error`, plus input guards.

use bollard::errors::Error as BollardError;
use ferry_core::{Error, Result};

/// HTTP status code of a daemon error response, if the error is one.
pub(crate) fn status(err: &BollardError) -> Option<u16> {
    match err {
        BollardError::DockerResponseServerError { status_code, .. } => Some(*status_code),
        _ => None,
    }
}

pub(crate) fn is_not_found(err: &BollardError) -> bool {
    status(err) == Some(404)
}

pub(crate) fn is_conflict(err: &BollardError) -> bool {
    status(err) == Some(409)
}

/// The daemon's message for API errors, otherwise the error and its sources
/// ("Error in the hyper legacy client: client error (Connect): Connection refused").
pub(crate) fn detail(err: &BollardError) -> String {
    match err {
        BollardError::DockerResponseServerError { status_code, message } => {
            let message = message.trim();
            if message.is_empty() { format!("HTTP {status_code}") } else { message.to_string() }
        }
        other => error_chain(other),
    }
}

/// `err: source: source-of-source`, skipping sources whose text is already included.
fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !text.is_empty() && !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

/// What a daemon 404 message says is missing: `No such image: x`,
/// `No such container: x`, `… network x not found`.
pub(crate) fn missing_subject(message: &str) -> Option<(&'static str, String)> {
    for (marker, kind) in [("No such image: ", "image"), ("No such container: ", "container")] {
        if let Some(i) = message.find(marker) {
            let name = message[i + marker.len()..].trim();
            if !name.is_empty() {
                return Some((kind, name.to_string()));
            }
        }
    }
    let mut rest = message;
    while let Some(i) = rest.find("network ") {
        let after = &rest[i + "network ".len()..];
        if let Some(end) = after.find(" not found") {
            let name = after[..end].trim();
            if !name.is_empty() && !name.contains(char::is_whitespace) {
                return Some(("network", name.to_string()));
            }
        }
        rest = after;
    }
    None
}

/// Map a bollard error: daemon 404 → `NotFound("<kind> '<id>'")` (kind and
/// name taken from the daemon message when it names what is missing, e.g. a
/// network when starting a container), 409 → `Conflict("<context>: <message>")`,
/// anything else → `Docker("<context>: <detail>")`.
pub(crate) fn map(err: BollardError, context: &str, kind: &str, id: &str) -> Error {
    match status(&err) {
        Some(404) => match missing_subject(&detail(&err)) {
            Some((kind, name)) => Error::not_found(kind, &name),
            None => Error::not_found(kind, id),
        },
        Some(409) => Error::Conflict(format!("{context}: {}", detail(&err))),
        _ => Error::Docker(format!("{context}: {}", detail(&err))),
    }
}

/// Like [`map`] but never produces `NotFound` (for operations where a 404 is
/// not about the named object).
pub(crate) fn map_docker(err: BollardError, context: &str) -> Error {
    match status(&err) {
        Some(409) => Error::Conflict(format!("{context}: {}", detail(&err))),
        _ => Error::Docker(format!("{context}: {}", detail(&err))),
    }
}

/// Validate a container / volume / network reference before it is put into a
/// URL path. Docker object names and ids match `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
pub(crate) fn check_object_ref(kind: &str, value: &str) -> Result<()> {
    let mut chars = value.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid { Ok(()) } else { Err(Error::invalid(format!("invalid {kind} reference '{value}'"))) }
}

/// Validate an image reference before it is put into a URL path.
pub(crate) fn check_image_ref(image: &str) -> Result<()> {
    let valid = !image.is_empty()
        && image.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':' | '@'))
        && image.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..");
    if valid { Ok(()) } else { Err(Error::invalid(format!("invalid image reference '{image}'"))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(status_code: u16, message: &str) -> BollardError {
        BollardError::DockerResponseServerError { status_code, message: message.to_string() }
    }

    #[test]
    fn maps_status_codes() {
        let e = map(server(404, "No such container: x"), "starting container x", "container", "x");
        assert!(matches!(&e, Error::NotFound(m) if m == "container 'x'"), "{e:?}");
        let e = map(server(409, "name in use"), "creating container x", "container", "x");
        assert!(matches!(&e, Error::Conflict(m) if m == "creating container x: name in use"), "{e:?}");
        let e = map(server(500, "boom"), "starting container x", "container", "x");
        assert!(matches!(&e, Error::Docker(m) if m == "starting container x: boom"), "{e:?}");
        let e = map(server(500, ""), "starting container x", "container", "x");
        assert!(matches!(&e, Error::Docker(m) if m == "starting container x: HTTP 500"), "{e:?}");
        let e = map(BollardError::RequestTimeoutError, "inspecting container x", "container", "x");
        assert!(matches!(&e, Error::Docker(m) if m == "inspecting container x: Timeout error"), "{e:?}");
        let e = map_docker(server(404, "network n not found"), "creating container x");
        assert!(matches!(&e, Error::Docker(_)), "{e:?}");
        let e = map(
            server(404, "failed to set up container networking: network ferrynet not found"),
            "starting container x",
            "container",
            "x",
        );
        assert!(matches!(&e, Error::NotFound(m) if m == "network 'ferrynet'"), "{e:?}");
        let e = map(server(404, "No such image: nope:1"), "creating container x", "image", "fallback");
        assert!(matches!(&e, Error::NotFound(m) if m == "image 'nope:1'"), "{e:?}");
        assert!(is_not_found(&server(404, "")));
        assert!(is_conflict(&server(409, "")));
        assert!(!is_conflict(&BollardError::RequestTimeoutError));
    }

    #[test]
    fn missing_subjects() {
        assert_eq!(missing_subject("No such container: abc"), Some(("container", "abc".into())));
        assert_eq!(missing_subject("No such image: ferry/web:dep-1"), Some(("image", "ferry/web:dep-1".into())));
        assert_eq!(
            missing_subject("failed to set up container networking: network ferry not found"),
            Some(("network", "ferry".into()))
        );
        assert_eq!(missing_subject("network ferry-net not found"), Some(("network", "ferry-net".into())));
        assert_eq!(missing_subject("page not found"), None);
        assert_eq!(missing_subject("network settings not available, container not found"), None);
        assert_eq!(missing_subject(""), None);
    }

    #[test]
    fn object_refs() {
        for ok in ["ferry-web-1a2b3c4d-9f8e7d", "abc123", "a.b_c-d", "0f1e"] {
            assert!(check_object_ref("container", ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", ".", "..", "a/b", "a?b", "a#b", "a b", "../etc", "é"] {
            assert!(check_object_ref("container", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn image_refs() {
        for ok in ["nginx", "nginx:alpine", "ferry/web:dep-1", "localhost:5000/a/b:1", "busybox@sha256:abc"] {
            assert!(check_image_ref(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a b", "a?b", "../x", "a/../b", "a//b", "/abs", "x#y"] {
            assert!(check_image_ref(bad).is_err(), "{bad}");
        }
    }
}
