//! Incremental Server-Sent Events parser (WHATWG `text/event-stream`).
//!
//! Bytes are fed in arbitrary chunks (as they arrive from the network); the
//! parser buffers partial lines — including a `\r\n` pair or a multi-byte UTF-8
//! character split across two chunks — and returns every event completed by
//! the chunk.
//!
//! Differences from a browser `EventSource`, on purpose:
//! * an event that has an `event:` name but no `data:` line is still
//!   dispatched (Ferry's `event: end` terminator carries no data);
//! * `retry:` is ignored (reconnection is handled by the caller).

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Event name (`event:` field); `message` when the event had none.
    pub event: String,
    /// `data:` lines joined with `\n` (no trailing newline).
    pub data: String,
    /// Last event id seen on the stream (`id:` field), if any.
    pub id: Option<String>,
}

/// Streaming parser state.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the current, not yet terminated line.
    line: Vec<u8>,
    /// The previous chunk ended with `\r`: a leading `\n` belongs to it.
    skip_lf: bool,
    /// No line has been processed yet (a UTF-8 BOM may start the stream).
    started: bool,
    event: String,
    data: String,
    has_data: bool,
    last_id: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the next chunk of the stream; returns the events it completed, in order.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let mut start = 0;
        for (i, &b) in chunk.iter().enumerate() {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    start = i + 1;
                    continue;
                }
            }
            if b == b'\n' || b == b'\r' {
                self.line.extend_from_slice(&chunk[start..i]);
                let line = std::mem::take(&mut self.line);
                if let Some(ev) = self.process_line(&line) {
                    events.push(ev);
                }
                self.skip_lf = b == b'\r';
                start = i + 1;
            }
        }
        self.line.extend_from_slice(&chunk[start..]);
        events
    }

    /// True when nothing is buffered (no partial line, no pending event).
    /// Per the spec, a pending event at end of stream is discarded.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.line.is_empty() && !self.has_data && self.event.is_empty()
    }

    fn process_line(&mut self, raw: &[u8]) -> Option<SseEvent> {
        let mut line = String::from_utf8_lossy(raw);
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix('\u{feff}') {
                line = rest.to_string().into();
            }
        }
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None; // comment (keep-alive)
        }
        let (field, value) = match line.find(':') {
            Some(pos) => {
                let value = &line[pos + 1..];
                (&line[..pos], value.strip_prefix(' ').unwrap_or(value))
            }
            None => (&line[..], ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
                self.has_data = true;
            }
            "id" if !value.contains('\0') => self.last_id = Some(value.to_string()),
            _ => {} // `retry` and unknown fields are ignored
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if !self.has_data && self.event.is_empty() {
            return None;
        }
        let mut data = std::mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        let event = std::mem::take(&mut self.event);
        self.has_data = false;
        Some(SseEvent {
            event: if event.is_empty() { "message".to_string() } else { event },
            data,
            id: self.last_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent { event: event.into(), data: data.into(), id: None }
    }

    fn parse_all(input: &[u8]) -> Vec<SseEvent> {
        SseParser::new().feed(input)
    }

    #[test]
    fn basic_events_with_lf() {
        let out = parse_all(b"event: log\ndata: {\"a\":1}\n\nevent: end\ndata: \n\n");
        assert_eq!(out, vec![ev("log", "{\"a\":1}"), ev("end", "")]);
    }

    #[test]
    fn crlf_and_lone_cr_line_endings() {
        assert_eq!(parse_all(b"event: log\r\ndata: x\r\n\r\n"), vec![ev("log", "x")]);
        assert_eq!(parse_all(b"event: log\rdata: y\r\r"), vec![ev("log", "y")]);
    }

    #[test]
    fn multi_line_data_is_joined_with_newlines() {
        let out = parse_all(b"data: first\ndata: second\ndata:\ndata: fourth\n\n");
        assert_eq!(out, vec![ev("message", "first\nsecond\n\nfourth")]);
    }

    #[test]
    fn comments_and_keepalives_are_ignored() {
        let out = parse_all(b":\n\n: keep-alive\n\nevent: log\n: inline comment\ndata: z\n\n");
        assert_eq!(out, vec![ev("log", "z")]);
    }

    #[test]
    fn event_without_data_is_dispatched() {
        // axum's `Event::default().event("end")` serializes exactly like this.
        assert_eq!(parse_all(b"event: end\n\n"), vec![ev("end", "")]);
    }

    #[test]
    fn blank_lines_alone_dispatch_nothing() {
        assert!(parse_all(b"\n\n\r\n\r\r").is_empty());
    }

    #[test]
    fn field_value_space_handling() {
        // Only one leading space is stripped; no colon means empty value.
        let out = parse_all(b"data:  two spaces\n\ndata\n\nevent:log\ndata:x\n\n");
        assert_eq!(out, vec![ev("message", " two spaces"), ev("message", ""), ev("log", "x")]);
    }

    #[test]
    fn id_persists_and_unknown_fields_ignored() {
        let out = parse_all(b"id: 7\nretry: 1000\nfoo: bar\ndata: a\n\ndata: b\n\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id.as_deref(), Some("7"));
        assert_eq!(out[1].id.as_deref(), Some("7"));
        assert_eq!(out[1].data, "b");
    }

    #[test]
    fn byte_at_a_time_feeding_matches_whole_input() {
        let input = "\u{feff}event: log\r\ndata: héllo wörld 🎉\r\n\r\n: ka\r\nevent: end\r\n\r\n".as_bytes();
        let whole = parse_all(input);
        let mut p = SseParser::new();
        let mut split = Vec::new();
        for b in input {
            split.extend(p.feed(std::slice::from_ref(b)));
        }
        assert_eq!(whole, split);
        assert_eq!(whole, vec![ev("log", "héllo wörld 🎉"), ev("end", "")]);
        assert!(p.is_idle());
    }

    #[test]
    fn crlf_split_across_chunks_is_one_line_ending() {
        let mut p = SseParser::new();
        let mut out = p.feed(b"data: a\r");
        out.extend(p.feed(b"\n"));
        out.extend(p.feed(b"\r"));
        out.extend(p.feed(b"\ndata: b\n\n"));
        assert_eq!(out, vec![ev("message", "a"), ev("message", "b")]);
    }

    #[test]
    fn incomplete_event_stays_pending() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: log\ndata: partial").is_empty());
        assert!(!p.is_idle());
        assert_eq!(p.feed(b" line\n\n"), vec![ev("log", "partial line")]);
        assert!(p.is_idle());
    }

    #[test]
    fn event_name_resets_between_events() {
        let out = parse_all(b"event: log\ndata: 1\n\ndata: 2\n\n");
        assert_eq!(out, vec![ev("log", "1"), ev("message", "2")]);
    }

    #[test]
    fn bom_only_stripped_at_stream_start() {
        let out = parse_all("data: a\n\n\u{feff}data: b\n\n".as_bytes());
        // A BOM in the middle is part of the field name, so the line is ignored.
        assert_eq!(out, vec![ev("message", "a")]);
    }
}
