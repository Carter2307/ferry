//! Container log streaming: Docker log chunks → `LogLine`s.
//!
//! Docker delivers logs as chunks (one multiplexed frame per log message, or
//! newline-split output for TTY containers). With `timestamps=true` every
//! message starts with an RFC3339Nano timestamp and a space. Lines longer than
//! the log driver's buffer (16 KiB) arrive as several messages, each with its
//! own timestamp; they are joined back into one line here.

use bollard::container::LogOutput;
use bollard::query_parameters::LogsOptionsBuilder;
use chrono::{DateTime, Utc};
use ferry_core::{LogLine, LogStream, LogStreamKind};
use futures::StreamExt;

/// A partial line is flushed once it grows beyond this many bytes (protects
/// against output that never contains a newline).
pub(crate) const MAX_LINE_BYTES: usize = 64 * 1024;

/// Longest RFC3339Nano timestamp Docker emits
/// (`2006-01-02T15:04:05.000000000+07:00`) plus some slack.
const MAX_TIMESTAMP_LEN: usize = 40;
/// Shortest RFC3339 timestamp (`2006-01-02T15:04:05Z`).
const MIN_TIMESTAMP_LEN: usize = 20;

/// Parse a leading `<RFC3339 timestamp> ` prefix; returns the time and the rest.
pub(crate) fn split_timestamp(chunk: &[u8]) -> Option<(DateTime<Utc>, &[u8])> {
    let window = &chunk[..chunk.len().min(MAX_TIMESTAMP_LEN + 1)];
    let space = window.iter().position(|b| *b == b' ')?;
    if space < MIN_TIMESTAMP_LEN {
        return None;
    }
    let text = std::str::from_utf8(&chunk[..space]).ok()?;
    let ts = DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&Utc);
    Some((ts, &chunk[space + 1..]))
}

/// A line being assembled for one output stream.
#[derive(Debug, Default)]
struct Partial {
    buf: Vec<u8>,
    ts: Option<DateTime<Utc>>,
    /// A line has started (its timestamp, if any, was consumed).
    open: bool,
}

impl Partial {
    fn take(&mut self, stream: LogStreamKind) -> LogLine {
        let mut bytes = std::mem::take(&mut self.buf);
        while bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let line = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        };
        let ts = self.ts.take().unwrap_or_else(Utc::now);
        self.open = false;
        LogLine { ts, stream, instance: None, line }
    }
}

/// Splits Docker log chunks into lines, buffering partial lines across chunks
/// (separately for stdout and stderr).
#[derive(Debug)]
pub(crate) struct LineSplitter {
    timestamps: bool,
    stdout: Partial,
    stderr: Partial,
}

impl LineSplitter {
    /// `timestamps`: the chunks carry Docker's leading timestamps.
    pub(crate) fn new(timestamps: bool) -> Self {
        LineSplitter { timestamps, stdout: Partial::default(), stderr: Partial::default() }
    }

    fn partial(&mut self, kind: LogStreamKind) -> &mut Partial {
        match kind {
            LogStreamKind::Stderr => &mut self.stderr,
            LogStreamKind::Stdout | LogStreamKind::System => &mut self.stdout,
        }
    }

    /// Feed one chunk; complete lines are appended to `out`.
    pub(crate) fn push(&mut self, kind: LogStreamKind, chunk: &[u8], out: &mut Vec<LogLine>) {
        if chunk.is_empty() {
            return;
        }
        let timestamps = self.timestamps;
        let partial = self.partial(kind);
        let mut rest = chunk;
        let mut chunk_start = true;
        loop {
            if timestamps {
                if !partial.open {
                    if let Some((ts, after)) = split_timestamp(rest) {
                        partial.ts = Some(ts);
                        rest = after;
                    }
                } else if chunk_start {
                    // Continuation of a message split by the log driver: it
                    // carries its own timestamp, the line keeps the first one.
                    if let Some((_, after)) = split_timestamp(rest) {
                        rest = after;
                    }
                }
            }
            partial.open = true;
            chunk_start = false;
            match rest.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    partial.buf.extend_from_slice(&rest[..i]);
                    out.push(partial.take(kind));
                    rest = &rest[i + 1..];
                    if rest.is_empty() {
                        break;
                    }
                }
                None => {
                    partial.buf.extend_from_slice(rest);
                    if partial.buf.len() >= MAX_LINE_BYTES {
                        out.push(partial.take(kind));
                    }
                    break;
                }
            }
        }
    }

    /// Flush unterminated lines (end of stream).
    pub(crate) fn finish(&mut self, out: &mut Vec<LogLine>) {
        for kind in [LogStreamKind::Stdout, LogStreamKind::Stderr] {
            let partial = self.partial(kind);
            if !partial.buf.is_empty() {
                out.push(partial.take(kind));
            }
            *partial = Partial::default();
        }
    }
}

/// Which stream a Docker chunk belongs to, and its bytes.
pub(crate) fn classify(output: LogOutput) -> (LogStreamKind, bytes::Bytes) {
    match output {
        LogOutput::StdErr { message } => (LogStreamKind::Stderr, message),
        LogOutput::StdOut { message } | LogOutput::Console { message } | LogOutput::StdIn { message } => {
            (LogStreamKind::Stdout, message)
        }
    }
}

/// `'static` stream of a container's log lines (see `Docker::logs`).
pub(crate) fn stream(client: bollard::Docker, id: String, follow: bool, tail: Option<usize>) -> LogStream {
    Box::pin(async_stream::stream! {
        let tail = tail.map_or_else(|| "all".to_string(), |n| n.to_string());
        let options = LogsOptionsBuilder::new()
            .follow(follow)
            .stdout(true)
            .stderr(true)
            .timestamps(true)
            .tail(&tail)
            .build();
        let mut chunks = client.logs(&id, Some(options));
        let mut splitter = LineSplitter::new(true);
        let mut lines = Vec::new();
        while let Some(item) = chunks.next().await {
            match item {
                Ok(output) => {
                    let (kind, bytes) = classify(output);
                    splitter.push(kind, &bytes, &mut lines);
                    for line in lines.drain(..) {
                        yield line;
                    }
                }
                Err(err) => {
                    tracing::debug!(container = %id, error = %crate::errors::detail(&err), "container log stream ended with an error");
                    break;
                }
            }
        }
        splitter.finish(&mut lines);
        for line in lines.drain(..) {
            yield line;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = "2026-09-25T10:11:12.123456789Z";

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(TS).unwrap().with_timezone(&Utc)
    }

    fn texts(lines: &[LogLine]) -> Vec<&str> {
        lines.iter().map(|l| l.line.as_str()).collect()
    }

    #[test]
    fn parses_timestamps() {
        let (t, rest) = split_timestamp(format!("{TS} hello").as_bytes()).map(|(t, r)| (t, r.to_vec())).unwrap();
        assert_eq!(t, ts());
        assert_eq!(rest, b"hello");
        let (t, rest) = split_timestamp(b"2026-09-25T12:11:12.5+02:00 x").unwrap();
        assert_eq!(t, DateTime::parse_from_rfc3339("2026-09-25T10:11:12.5Z").unwrap());
        assert_eq!(rest, b"x");
        let (_, rest) = split_timestamp(b"2026-09-25T10:11:12Z ").unwrap();
        assert!(rest.is_empty());
        assert!(split_timestamp(b"hello world").is_none());
        assert!(split_timestamp(b"2026-09-25T10:11:12.1Z").is_none(), "no separating space");
        assert!(split_timestamp(b"not-a-timestamp-at-all-but-long x").is_none());
        assert!(split_timestamp(b"").is_none());
    }

    #[test]
    fn splits_lines_with_timestamps() {
        let mut s = LineSplitter::new(true);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, format!("{TS} hello\n").as_bytes(), &mut out);
        s.push(LogStreamKind::Stderr, format!("{TS} oops\r\n").as_bytes(), &mut out);
        s.push(LogStreamKind::Stdout, format!("{TS} \n").as_bytes(), &mut out);
        assert_eq!(texts(&out), ["hello", "oops", ""]);
        assert_eq!(out[0].stream, LogStreamKind::Stdout);
        assert_eq!(out[1].stream, LogStreamKind::Stderr);
        assert!(out.iter().all(|l| l.ts == ts() && l.instance.is_none()));
    }

    #[test]
    fn several_lines_in_one_chunk() {
        let mut s = LineSplitter::new(true);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, format!("{TS} a\n2026-09-25T10:11:13Z b\n").as_bytes(), &mut out);
        assert_eq!(texts(&out), ["a", "b"]);
        assert_eq!(out[0].ts, ts());
        assert_eq!(out[1].ts, DateTime::parse_from_rfc3339("2026-09-25T10:11:13Z").unwrap());
    }

    #[test]
    fn joins_partial_messages() {
        // A long line split by the log driver: each part has its own timestamp.
        let mut s = LineSplitter::new(true);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, format!("{TS} first-half ").as_bytes(), &mut out);
        assert!(out.is_empty());
        s.push(LogStreamKind::Stdout, b"2026-09-25T10:11:14Z second-half\n", &mut out);
        assert_eq!(texts(&out), ["first-half second-half"]);
        assert_eq!(out[0].ts, ts(), "a joined line keeps the first timestamp");
    }

    #[test]
    fn keeps_streams_apart() {
        let mut s = LineSplitter::new(true);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, format!("{TS} out-").as_bytes(), &mut out);
        s.push(LogStreamKind::Stderr, format!("{TS} err\n").as_bytes(), &mut out);
        s.push(LogStreamKind::Stdout, format!("{TS} line\n").as_bytes(), &mut out);
        assert_eq!(texts(&out), ["err", "out-line"]);
        assert_eq!(out[0].stream, LogStreamKind::Stderr);
        assert_eq!(out[1].stream, LogStreamKind::Stdout);
    }

    #[test]
    fn flushes_on_finish_and_falls_back_to_now() {
        let before = Utc::now();
        let mut s = LineSplitter::new(true);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, b"no timestamp and no newline", &mut out);
        s.push(LogStreamKind::Stderr, format!("{TS} tail\r").as_bytes(), &mut out);
        assert!(out.is_empty());
        s.finish(&mut out);
        assert_eq!(texts(&out), ["no timestamp and no newline", "tail"]);
        assert!(out[0].ts >= before);
        assert_eq!(out[1].ts, ts());
        let mut again = Vec::new();
        s.finish(&mut again);
        assert!(again.is_empty());
    }

    #[test]
    fn without_timestamps() {
        let mut s = LineSplitter::new(false);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, format!("{TS} kept\npart").as_bytes(), &mut out);
        s.push(LogStreamKind::Stdout, b"ial\n", &mut out);
        assert_eq!(texts(&out), [format!("{TS} kept").as_str(), "partial"]);
    }

    #[test]
    fn caps_unterminated_lines() {
        let mut s = LineSplitter::new(false);
        let mut out = Vec::new();
        let big = vec![b'x'; MAX_LINE_BYTES / 2 + 1];
        s.push(LogStreamKind::Stdout, &big, &mut out);
        assert!(out.is_empty());
        s.push(LogStreamKind::Stdout, &big, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line.len(), big.len() * 2);
        s.push(LogStreamKind::Stdout, b"rest\n", &mut out);
        assert_eq!(out[1].line, "rest");
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let mut s = LineSplitter::new(false);
        let mut out = Vec::new();
        s.push(LogStreamKind::Stdout, b"a\xffb\n", &mut out);
        assert_eq!(out[0].line, "a\u{fffd}b");
    }

    #[test]
    fn classifies_outputs() {
        let msg = bytes::Bytes::from_static(b"x");
        assert_eq!(classify(LogOutput::StdErr { message: msg.clone() }).0, LogStreamKind::Stderr);
        assert_eq!(classify(LogOutput::StdOut { message: msg.clone() }).0, LogStreamKind::Stdout);
        assert_eq!(classify(LogOutput::Console { message: msg }).0, LogStreamKind::Stdout);
    }
}
