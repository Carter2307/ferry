//! Body types used on both sides of the proxy. Bodies are streamed frame by
//! frame (never collected), so there is no size limit and no buffering.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Frame, SizeHint};

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

/// `body`, keeping `guard` alive until hyper is done with it (sent or dropped).
pub(crate) fn with_guard<G: Send + Unpin + 'static>(body: ProxyBody, guard: G) -> ProxyBody {
    Guarded { body, _guard: guard }.boxed_unsync()
}

struct Guarded<G> {
    body: ProxyBody,
    _guard: G,
}

impl<G> Body for Guarded<G>
where
    G: Unpin,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
