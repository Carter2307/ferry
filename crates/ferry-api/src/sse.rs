//! Server-Sent Events framing for log streams (DESIGN.md §9).
//!
//! Every line is `event: log` + `data: <LogLine JSON>`. Finite streams (deploy
//! and job logs, runtime logs without `follow`) end with `event: end` and an
//! empty `data:` field — the empty data line matters: browsers only dispatch
//! events that carry a data field. A `:` comment is sent after 15 seconds
//! without output so proxies and clients keep the connection open.
//!
//! The framing is written by hand (instead of `axum::response::Sse`) because
//! axum drops empty data fields, which would make `end` invisible to
//! `EventSource`.
//!
//! Every stream also ends (without `event: end`: the log isn't complete) as
//! soon as the server shuts down, so an open follower can never hold up the
//! HTTP server's graceful shutdown.

use std::convert::Infallible;
use std::time::Duration;

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use ferry_core::{CancellationToken, LogLine, LogStream};
use futures::StreamExt;
use http::{HeaderValue, header};

/// Interval of keep-alive comments while a stream is idle.
pub const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// The keep-alive comment frame.
pub const KEEP_ALIVE_FRAME: &[u8] = b": keep-alive\n\n";

/// The terminating frame of finite streams.
pub const END_FRAME: &[u8] = b"event: end\ndata: \n\n";

/// Frame one log line: `event: log\ndata: {json}\n\n`. Compact JSON never
/// contains raw newlines (they are escaped inside strings), so one `data:`
/// field is always enough.
pub fn log_frame(line: &LogLine) -> Option<Bytes> {
    match serde_json::to_string(line) {
        Ok(json) => Some(Bytes::from(format!("event: log\ndata: {json}\n\n"))),
        Err(e) => {
            tracing::debug!("dropping unserializable log line: {e}");
            None
        }
    }
}

enum Phase {
    Streaming,
    Done,
}

/// Turn a log stream into an SSE HTTP response. `finite` streams get a final
/// `event: end` frame when the log stream completes. The response body ends
/// when `shutdown` is cancelled.
pub fn log_response(stream: LogStream, finite: bool, shutdown: &CancellationToken) -> Response {
    let shutdown = shutdown.clone();
    let state = (stream, Phase::Streaming, shutdown);
    let frames = futures::stream::unfold(state, move |(mut stream, phase, shutdown)| async move {
        if matches!(phase, Phase::Done) {
            return None;
        }
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => return None,
                item = stream.next() => match item {
                    Some(line) => {
                        if let Some(frame) = log_frame(&line) {
                            return Some((Ok::<Bytes, Infallible>(frame), (stream, Phase::Streaming, shutdown)));
                        }
                    }
                    None if finite => {
                        return Some((Ok(Bytes::from_static(END_FRAME)), (stream, Phase::Done, shutdown)));
                    }
                    None => return None,
                },
                _ = tokio::time::sleep(KEEP_ALIVE) => {
                    return Some((Ok(Bytes::from_static(KEEP_ALIVE_FRAME)), (stream, Phase::Streaming, shutdown)));
                }
            }
        }
    });
    let mut resp = Body::from_stream(frames).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // Ask reverse proxies (nginx) not to buffer the stream.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::LogStreamKind;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn frames_finite_stream() {
        let lines = vec![LogLine::system("==> hi"), LogLine::new(LogStreamKind::Stdout, "a\nb")];
        let resp = log_response(Box::pin(futures::stream::iter(lines)), true, &CancellationToken::new());
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "text/event-stream");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let events: Vec<&str> = text.split("\n\n").filter(|s| !s.is_empty()).collect();
        assert_eq!(events.len(), 3, "{text}");
        assert!(events[0].starts_with("event: log\ndata: {"));
        assert!(events[1].contains(r#""line":"a\nb""#));
        assert_eq!(events[2], "event: end\ndata: ");
    }

    #[tokio::test]
    async fn infinite_stream_has_no_end() {
        let resp =
            log_response(Box::pin(futures::stream::iter(vec![LogLine::system("x")])), false, &CancellationToken::new());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains("event: end"));
        assert!(text.starts_with("event: log\n"));
    }

    #[tokio::test(start_paused = true)]
    async fn keep_alive_while_idle() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LogLine>();
        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        let resp = log_response(Box::pin(stream), true, &CancellationToken::new());
        let mut body = resp.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert_eq!(&first[..], KEEP_ALIVE_FRAME);
        tx.send(LogLine::system("late")).unwrap();
        let second = body.next().await.unwrap().unwrap();
        assert!(second.starts_with(b"event: log\n"));
        drop(tx);
        let end = body.next().await.unwrap().unwrap();
        assert_eq!(&end[..], END_FRAME);
        assert!(body.next().await.is_none());
    }

    #[tokio::test]
    async fn shutdown_ends_an_open_stream() {
        // A followed log that never ends on its own (e.g. a queued deploy).
        let stream = futures::stream::iter(vec![LogLine::system("first")]).chain(futures::stream::pending());
        let token = CancellationToken::new();
        let resp = log_response(Box::pin(stream), true, &token);
        let mut body = resp.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert!(first.starts_with(b"event: log\n"));
        token.cancel();
        let rest = tokio::time::timeout(Duration::from_secs(1), body.next()).await.expect("the stream must end");
        assert!(rest.is_none(), "no end frame: the log isn't complete");

        // Cancelled before the first poll: nothing at all.
        let resp = log_response(Box::pin(futures::stream::pending()), false, &token);
        let mut body = resp.into_body().into_data_stream();
        assert!(tokio::time::timeout(Duration::from_secs(1), body.next()).await.unwrap().is_none());
    }
}
