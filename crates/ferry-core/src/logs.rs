//! Log lines and sinks shared by the builder, runtime, engine and API.
//!
//! Wire format: the API streams logs as Server-Sent Events. Each event is
//! `event: log` with `data: <LogLine as JSON>`; a final `event: end` (empty
//! data) is sent when a finite stream (deploy/job logs) completes.

use std::pin::Pin;

use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Which output a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStreamKind {
    Stdout,
    Stderr,
    /// Ferry's own messages ("==> Cloning repo", "==> Deploy live").
    System,
}

/// One line of output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub ts: DateTime<Utc>,
    pub stream: LogStreamKind,
    /// Short container/instance identifier for runtime logs (None for build logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub line: String,
}

impl LogLine {
    pub fn new(stream: LogStreamKind, line: impl Into<String>) -> Self {
        LogLine { ts: Utc::now(), stream, instance: None, line: line.into() }
    }

    pub fn system(line: impl Into<String>) -> Self {
        Self::new(LogStreamKind::System, line)
    }

    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }
}

/// A boxed stream of log lines.
pub type LogStream = Pin<Box<dyn Stream<Item = LogLine> + Send + 'static>>;

/// Cheap, cloneable handle that producers (builder, engine) write lines into.
/// Writing never blocks and never fails; lines are dropped if nobody listens.
#[derive(Clone, Debug)]
pub struct LogSink {
    tx: Option<mpsc::UnboundedSender<LogLine>>,
}

impl LogSink {
    /// A sink forwarding every line into `tx`.
    pub fn new(tx: mpsc::UnboundedSender<LogLine>) -> Self {
        LogSink { tx: Some(tx) }
    }

    /// A sink that discards everything.
    pub fn noop() -> Self {
        LogSink { tx: None }
    }

    /// A sink plus the receiver collecting its lines (handy in tests).
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<LogLine>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (LogSink::new(tx), rx)
    }

    pub fn send(&self, line: LogLine) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(line);
        }
    }

    pub fn line(&self, stream: LogStreamKind, text: impl Into<String>) {
        self.send(LogLine::new(stream, text));
    }

    /// Ferry message, conventionally prefixed with `==> `.
    pub fn system(&self, text: impl Into<String>) {
        self.line(LogStreamKind::System, text);
    }

    pub fn stdout(&self, text: impl Into<String>) {
        self.line(LogStreamKind::Stdout, text);
    }

    pub fn stderr(&self, text: impl Into<String>) {
        self.line(LogStreamKind::Stderr, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_forwards_and_serializes() {
        let (sink, mut rx) = LogSink::channel();
        sink.system("==> hello");
        sink.stderr("oops");
        let a = rx.try_recv().unwrap();
        assert_eq!(a.stream, LogStreamKind::System);
        let b = rx.try_recv().unwrap();
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["stream"], "stderr");
        assert!(json.get("instance").is_none());
        LogSink::noop().system("dropped");
    }
}
