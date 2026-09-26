//! Body types used on both sides of the proxy. Bodies are streamed frame by
//! frame (never collected), so there is no size limit and no buffering.

use std::error::Error as StdError;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Frame, Incoming, SizeHint};
use tokio::time::{Instant, Sleep};

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
pub(crate) fn stream(body: Incoming) -> ProxyBody {
    body.map_err(BoxError::from).boxed_unsync()
}

/// Wrap a client's request body, failing it with [`BodyTimedOut`] when the
/// client sends nothing for `timeout` while the proxy is waiting for more.
///
/// Only time spent waiting on the client counts: the clock starts when a
/// read finds no data and stops at the next frame, so an upstream that is
/// slow to accept the body (back-pressure) never trips it, and neither does
/// anything that happens once the body is complete (the response, however
/// long it streams).
pub(crate) fn with_idle_timeout<B>(body: B, timeout: Duration) -> ProxyBody
where
    B: Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: Into<BoxError>,
{
    IdleTimeout { body, timeout, timer: None, waiting: false, expired: false }.boxed_unsync()
}

/// Error of a request body whose client stopped sending (see
/// [`with_idle_timeout`]).
#[derive(Debug)]
pub(crate) struct BodyTimedOut(Duration);

impl std::fmt::Display for BodyTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "client sent no request body data for {}s", self.0.as_secs_f32())
    }
}

impl StdError for BodyTimedOut {}

impl BodyTimedOut {
    /// Whether `err` (or any error it wraps) is a request body timeout.
    pub(crate) fn caused(err: &(dyn StdError + 'static)) -> bool {
        let mut next = Some(err);
        while let Some(e) = next {
            if e.is::<BodyTimedOut>() {
                return true;
            }
            next = e.source();
        }
        false
    }
}

struct IdleTimeout<B> {
    body: B,
    timeout: Duration,
    /// Created on the first wait, then re-armed in place.
    timer: Option<Pin<Box<Sleep>>>,
    /// The timer is armed for the current wait.
    waiting: bool,
    /// Once timed out, the body keeps failing: it must never look complete.
    expired: bool,
}

impl<B> Body for IdleTimeout<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<BoxError>,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        if this.expired {
            return Poll::Ready(Some(Err(Box::new(BodyTimedOut(this.timeout)))));
        }
        if let Poll::Ready(frame) = Pin::new(&mut this.body).poll_frame(cx) {
            this.waiting = false;
            return Poll::Ready(frame.map(|res| res.map_err(Into::into)));
        }
        // No data: (re)start the clock unless this wait already started it.
        if !this.waiting {
            let deadline = Instant::now() + this.timeout;
            match &mut this.timer {
                Some(timer) => timer.as_mut().reset(deadline),
                None => this.timer = Some(Box::pin(tokio::time::sleep_until(deadline))),
            }
            this.waiting = true;
        }
        let expired = this.timer.as_mut().is_some_and(|timer| timer.as_mut().poll(cx).is_ready());
        if !expired {
            return Poll::Pending;
        }
        this.expired = true;
        Poll::Ready(Some(Err(Box::new(BodyTimedOut(this.timeout)))))
    }

    fn is_end_stream(&self) -> bool {
        !self.expired && self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
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

#[cfg(test)]
mod tests {
    use futures::channel::mpsc;
    use http_body_util::StreamBody;

    use super::*;

    type Sender = mpsc::UnboundedSender<Result<Frame<Bytes>, BoxError>>;

    fn send_after(tx: &Sender, delay: Duration, data: &'static str) {
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.unbounded_send(Ok(Frame::data(Bytes::from_static(data.as_bytes()))));
        });
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_only_counts_waits_for_the_client() {
        let timeout = Duration::from_secs(60);
        let (tx, rx) = mpsc::unbounded();
        let mut body = with_idle_timeout(StreamBody::new(rx), timeout);

        // Pieces arriving just within the timeout, for 5 times as long.
        for _ in 0..5 {
            send_after(&tx, timeout - Duration::from_secs(1), "x");
            assert_eq!(body.frame().await.unwrap().unwrap().into_data().unwrap(), "x");
        }

        // Not read for an hour (the upstream applying back-pressure): the
        // clock only starts once the proxy asks for more and finds nothing.
        tokio::time::sleep(Duration::from_secs(3600)).await;
        send_after(&tx, timeout - Duration::from_secs(1), "y");
        assert_eq!(body.frame().await.unwrap().unwrap().into_data().unwrap(), "y");

        // Then nothing: fails after exactly the timeout...
        let started = Instant::now();
        let err = body.frame().await.unwrap().unwrap_err();
        assert_eq!(started.elapsed(), timeout);
        assert!(BodyTimedOut::caused(err.as_ref()), "{err}");
        assert!(err.to_string().contains("60s"), "{err}");

        // ...and keeps failing: a timed-out body must never look complete
        // (a chunked upstream request would end cleanly, truncated).
        drop(tx);
        assert!(!body.is_end_stream());
        assert!(body.frame().await.unwrap().is_err());
    }

    /// Wraps an error the way hyper's errors wrap a body error (as `source`).
    #[derive(Debug)]
    struct Wrapper(BoxError);

    impl std::fmt::Display for Wrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("wrapper")
        }
    }

    impl StdError for Wrapper {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn finds_body_timeouts_in_error_chains() {
        let timed_out = || -> BoxError { Box::new(BodyTimedOut(Duration::from_secs(1))) };
        assert!(BodyTimedOut::caused(timed_out().as_ref()));
        assert!(BodyTimedOut::caused(&Wrapper(Box::new(Wrapper(timed_out())))));
        assert!(!BodyTimedOut::caused(&Wrapper("unrelated".into())));
    }
}
