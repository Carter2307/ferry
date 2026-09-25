//! Body types used on both sides of the proxy. Bodies are streamed frame by
//! frame (never collected), so there is no size limit and no buffering.

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full};

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Body of responses sent to clients and of requests sent upstream.
pub(crate) type ProxyBody = UnsyncBoxBody<Bytes, BoxError>;

pub(crate) fn empty() -> ProxyBody {
    Empty::<Bytes>::new().map_err(|never| match never {}).boxed_unsync()
}

pub(crate) fn full(data: impl Into<Bytes>) -> ProxyBody {
    Full::new(data.into()).map_err(|never| match never {}).boxed_unsync()
}

/// Wrap a hyper body (client request or upstream response) without buffering.
pub(crate) fn stream(body: hyper::body::Incoming) -> ProxyBody {
    body.map_err(BoxError::from).boxed_unsync()
}
