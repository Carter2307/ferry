//! Terminal output: colors, aligned tables, key/value blocks, relative times,
//! human-readable sizes and durations, and log line formatting.
//!
//! Colors are used only when the target stream is a TTY, `NO_COLOR` is unset
//! and `TERM` is not `dumb`.

use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Local, Utc};
use ferry_core::resources::{format_cpus, format_memory_mb};
use ferry_core::{DatastoreStatus, DeployStatus, JobStatus, LogLine, LogStreamKind, ServiceState, ServiceType};

/// Print a line to stdout, returning an `io::Result` instead of panicking on
/// a closed pipe (`ferry logs web | head`).
macro_rules! outln {
    () => {
        $crate::output::write_stdout(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::output::write_stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// Print a line to stderr (progress notes, warnings). Write errors are ignored.
macro_rules! errln {
    () => {
        $crate::output::write_stderr(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::output::write_stderr(format_args!("{}\n", format_args!($($arg)*)))
    };
}

pub(crate) use {errln, outln};

pub fn write_stdout(args: fmt::Arguments<'_>) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_fmt(args)?;
    out.flush()
}

pub fn write_stderr(args: fmt::Arguments<'_>) {
    let mut err = io::stderr().lock();
    let _ = err.write_fmt(args);
    let _ = err.flush();
}

/// ANSI styles used by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Bold,
    Dim,
    Red,
    Green,
    Yellow,
    Cyan,
    BoldCyan,
}

impl Color {
    fn code(self) -> &'static str {
        match self {
            Color::Bold => "1",
            Color::Dim => "2",
            Color::Red => "31",
            Color::Green => "32",
            Color::Yellow => "33",
            Color::Cyan => "36",
            Color::BoldCyan => "1;36",
        }
    }
}

/// Wrap `text` in ANSI codes when `enabled`.
pub fn paint(enabled: bool, color: Color, text: &str) -> String {
    if enabled && !text.is_empty() { format!("\x1b[{}m{text}\x1b[0m", color.code()) } else { text.to_string() }
}

/// Colors allowed by the environment (`NO_COLOR` unset, `TERM` not `dumb`).
fn env_allows_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::env::var_os("TERM").is_none_or(|t| t != "dumb")
}

/// Whether stdout gets colors (TTY + environment), computed once.
pub fn stdout_color() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| io::stdout().is_terminal() && env_allows_color())
}

/// Whether stderr gets colors (TTY + environment), computed once.
pub fn stderr_color() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| io::stderr().is_terminal() && env_allows_color())
}

/// Paint for stdout.
pub fn out(color: Color, text: &str) -> String {
    paint(stdout_color(), color, text)
}

/// Paint for stderr.
pub fn err(color: Color, text: &str) -> String {
    paint(stderr_color(), color, text)
}

/// Message printed when the user interrupts a follow (Ctrl-C), e.g. how to
/// resume following a deploy that keeps running on the server.
static INTERRUPT_HINT: Mutex<Option<String>> = Mutex::new(None);

pub fn set_interrupt_hint(hint: Option<String>) {
    if let Ok(mut h) = INTERRUPT_HINT.lock() {
        *h = hint;
    }
}

pub fn take_interrupt_hint() -> Option<String> {
    INTERRUPT_HINT.lock().ok().and_then(|mut h| h.take())
}

// ---------------------------------------------------------------------------
// Tables & key/value blocks

/// One table cell: plain text plus an optional style (applied after padding
/// is computed, so colors never break the alignment).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    text: String,
    color: Option<Color>,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Cell { text: sanitize(&text.into()), color: None }
    }

    pub fn colored(text: impl Into<String>, color: Option<Color>) -> Self {
        Cell { text: sanitize(&text.into()), color }
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::new(s)
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::new(s)
    }
}

/// Replace control characters (newlines, escape sequences from server data)
/// so a cell always renders on one line without affecting the terminal.
fn sanitize(s: &str) -> String {
    if s.chars().any(char::is_control) {
        s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    } else {
        s.to_string()
    }
}

fn width(s: &str) -> usize {
    s.chars().count()
}

/// Aligned plain-text table with a header row.
#[derive(Debug, Clone, Default)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

const COLUMN_GAP: usize = 3;

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Table { headers: headers.iter().map(|h| h.to_string()).collect(), rows: Vec::new() }
    }

    pub fn row(&mut self, cells: Vec<Cell>) {
        self.rows.push(cells);
    }

    /// Render with `color` controlling ANSI styles. Lines have no trailing
    /// whitespace; the output ends with a newline.
    pub fn render(&self, color: bool) -> String {
        let ncols = self.headers.len().max(self.rows.iter().map(Vec::len).max().unwrap_or(0));
        let mut widths = vec![0usize; ncols];
        for (i, h) in self.headers.iter().enumerate() {
            widths[i] = width(h);
        }
        for row in &self.rows {
            for (i, c) in row.iter().enumerate() {
                widths[i] = widths[i].max(width(&c.text));
            }
        }
        let empty = Cell::default();
        let mut out = String::new();
        let header_cells: Vec<Cell> =
            self.headers.iter().map(|h| Cell { text: h.clone(), color: Some(Color::Bold) }).collect();
        let lines = std::iter::once(&header_cells).chain(self.rows.iter());
        for row in lines {
            let mut line = String::new();
            for (i, w) in widths.iter().enumerate() {
                let cell = row.get(i).unwrap_or(&empty);
                let text = match cell.color {
                    Some(c) => paint(color, c, &cell.text),
                    None => cell.text.clone(),
                };
                line.push_str(&text);
                if i + 1 < ncols {
                    line.push_str(&" ".repeat(w - width(&cell.text) + COLUMN_GAP));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// Render `key: value` lines with aligned values (keys bold when `color`).
pub fn render_kv(pairs: &[(&str, Cell)], color: bool) -> String {
    let w = pairs.iter().map(|(k, _)| width(k)).max().unwrap_or(0) + 1;
    let mut out = String::new();
    for (k, v) in pairs {
        let key = format!("{k}:");
        let pad = " ".repeat(w - width(&key) + 2);
        out.push_str(&paint(color, Color::Bold, &key));
        out.push_str(&pad);
        let text = v.text.trim_end();
        match v.color {
            Some(c) => out.push_str(&paint(color, c, text)),
            None => out.push_str(text),
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Formatting helpers

/// `"3m ago"`, `"2h ago"`, `"just now"`, `"in 5m"`.
pub fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    if secs.abs() < 5 {
        "just now".to_string()
    } else if secs < 0 {
        format!("in {}", short_span(-secs))
    } else {
        format!("{} ago", short_span(secs))
    }
}

fn short_span(secs: i64) -> String {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    match secs {
        s if s < MIN => format!("{s}s"),
        s if s < HOUR => format!("{}m", s / MIN),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < 30 * DAY => format!("{}d", s / DAY),
        s if s < 365 * DAY => format!("{}mo", s / (30 * DAY)),
        s => format!("{}y", s / (365 * DAY)),
    }
}

/// Relative time for an optional timestamp (`-` when absent).
pub fn ago(t: Option<DateTime<Utc>>) -> String {
    t.map(|t| relative_time(t, Utc::now())).unwrap_or_else(|| "-".to_string())
}

/// `"42s"`, `"3m 12s"`, `"1h 05m"`.
pub fn human_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Duration between `start` and `end` (or now while still running).
pub fn span(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    match start {
        Some(s) => human_duration((end.unwrap_or_else(Utc::now) - s).num_seconds()),
        None => "-".to_string(),
    }
}

/// `"512 B"`, `"1.5 KiB"`, `"12.3 MiB"`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A container's configured memory limit (bytes) in the words of
/// `ferry show` (`512 MiB`, `1 GiB`) when it is a whole number of MiB.
pub fn memory_limit_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    match u32::try_from(bytes / MIB) {
        Ok(mb) if mb > 0 && bytes.is_multiple_of(MIB) => format_memory_mb(mb),
        _ => human_bytes(bytes),
    }
}

/// A server default memory limit (MiB): `512 MiB`, or `unlimited` for 0.
pub fn memory_or_unlimited(mb: u32) -> String {
    if mb == 0 { "unlimited".to_string() } else { format_memory_mb(mb) }
}

/// A server default CPU limit: `1 CPU`, or `unlimited` for 0.
pub fn cpus_or_unlimited(cpus: f64) -> String {
    if cpus > 0.0 { format_cpus(cpus) } else { "unlimited".to_string() }
}

/// The memory limit of a service or datastore: its own (`explicit`), else
/// the server default (`default`, 0 = unlimited; `None` when the server
/// didn't say what it is).
pub fn memory_limit(explicit: Option<u32>, default: Option<u32>) -> String {
    match explicit.filter(|m| *m > 0) {
        Some(mb) => format_memory_mb(mb),
        None => server_default(default.map(memory_or_unlimited)),
    }
}

/// The CPU limit of a service or datastore (see [`memory_limit`]).
pub fn cpu_limit(explicit: Option<f64>, default: Option<f64>) -> String {
    match explicit.filter(|c| *c > 0.0) {
        Some(cpus) => format_cpus(cpus),
        None => server_default(default.map(cpus_or_unlimited)),
    }
}

fn server_default(value: Option<String>) -> String {
    match value.as_deref() {
        Some("unlimited") => "unlimited (server default)".to_string(),
        Some(v) => format!("server default ({v})"),
        None => "server default".to_string(),
    }
}

/// Truncate to `max` characters, ending with `…` when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.push('…');
    t
}

/// Short user-facing service type name (`web`, `pserv`, `worker`, `cron`, `static`).
pub fn type_short(t: ServiceType) -> &'static str {
    match t {
        ServiceType::WebService => "web",
        ServiceType::PrivateService => "pserv",
        ServiceType::BackgroundWorker => "worker",
        ServiceType::CronJob => "cron",
        ServiceType::StaticSite => "static",
    }
}

/// Human service type name ("web service", "cron job", ...).
pub fn type_long(t: ServiceType) -> &'static str {
    match t {
        ServiceType::WebService => "web service",
        ServiceType::PrivateService => "private service",
        ServiceType::BackgroundWorker => "background worker",
        ServiceType::CronJob => "cron job",
        ServiceType::StaticSite => "static site",
    }
}

pub fn service_state_color(s: ServiceState) -> Option<Color> {
    match s {
        ServiceState::Live => Some(Color::Green),
        ServiceState::Deploying | ServiceState::Degraded => Some(Color::Yellow),
        ServiceState::Failed => Some(Color::Red),
        ServiceState::Suspended | ServiceState::NotDeployed => Some(Color::Dim),
    }
}

pub fn deploy_status_color(s: DeployStatus) -> Option<Color> {
    match s {
        DeployStatus::Live => Some(Color::Green),
        DeployStatus::Queued | DeployStatus::Building | DeployStatus::Deploying => Some(Color::Yellow),
        DeployStatus::BuildFailed | DeployStatus::DeployFailed => Some(Color::Red),
        DeployStatus::Deactivated | DeployStatus::Canceled => Some(Color::Dim),
    }
}

pub fn job_status_color(s: JobStatus) -> Option<Color> {
    match s {
        JobStatus::Succeeded => Some(Color::Green),
        JobStatus::Pending | JobStatus::Running => Some(Color::Yellow),
        JobStatus::Failed => Some(Color::Red),
        JobStatus::Canceled => Some(Color::Dim),
    }
}

pub fn datastore_status_color(s: DatastoreStatus) -> Option<Color> {
    match s {
        DatastoreStatus::Available => Some(Color::Green),
        DatastoreStatus::Creating => Some(Color::Yellow),
        DatastoreStatus::Failed => Some(Color::Red),
    }
}

/// Paint `text` with an optional color for stdout.
pub fn out_opt(color: Option<Color>, text: &str) -> String {
    match color {
        Some(c) => out(c, text),
        None => text.to_string(),
    }
}

/// `HH:MM:SS [instance] text` (local time). System lines (`==> ...`) are
/// emphasized when `color` is on.
pub fn format_log_line(line: &LogLine, color: bool) -> String {
    let ts = line.ts.with_timezone(&Local).format("%H:%M:%S").to_string();
    let mut s = paint(color, Color::Dim, &ts);
    s.push(' ');
    if let Some(inst) = line.instance.as_deref().filter(|i| !i.is_empty()) {
        s.push_str(&paint(color, Color::Cyan, &format!("[{inst}]")));
        s.push(' ');
    }
    let text = line.line.trim_end_matches(['\n', '\r']);
    if line.stream == LogStreamKind::System || text.starts_with("==>") {
        s.push_str(&paint(color, Color::BoldCyan, text));
    } else {
        s.push_str(text);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn table_aligns_columns_and_trims_trailing_space() {
        let mut t = Table::new(&["NAME", "TYPE", "URL"]);
        t.row(vec!["web".into(), "web".into(), "http://web.localhost:8080".into()]);
        t.row(vec!["background".into(), "worker".into(), "".into()]);
        let out = t.render(false);
        let expected = "\
NAME         TYPE     URL
web          web      http://web.localhost:8080
background   worker
";
        assert_eq!(out, expected);
    }

    #[test]
    fn table_colors_do_not_break_alignment() {
        let mut t = Table::new(&["STATE", "X"]);
        t.row(vec![Cell::colored("live", Some(Color::Green)), "1".into()]);
        t.row(vec![Cell::colored("failed", Some(Color::Red)), "2".into()]);
        let colored = t.render(true);
        let stripped = strip_ansi(&colored);
        assert_eq!(stripped, t.render(false));
        assert!(colored.contains("\x1b[32mlive\x1b[0m"));
        assert!(colored.contains("\x1b[1mSTATE\x1b[0m"));
    }

    #[test]
    fn table_handles_ragged_rows_and_control_chars() {
        let mut t = Table::new(&["A", "B"]);
        t.row(vec!["x\ny".into()]);
        t.row(vec!["1".into(), "2".into(), "extra".into()]);
        let out = t.render(false);
        assert_eq!(out, "A     B\nx y\n1     2   extra\n");
    }

    #[test]
    fn unicode_width_counts_chars() {
        let mut t = Table::new(&["N", "M"]);
        t.row(vec!["héllo".into(), "x".into()]);
        assert_eq!(t.render(false), "N       M\nhéllo   x\n");
    }

    #[test]
    fn kv_block_is_aligned() {
        let pairs = [("Name", Cell::from("web")), ("Deploy hook", Cell::colored("/hooks/x\n", Some(Color::Red)))];
        assert_eq!(render_kv(&pairs, false), "Name:         web\nDeploy hook:  /hooks/x\n");
        let colored = render_kv(&pairs, true);
        assert!(colored.contains("\x1b[1mName:\x1b[0m"));
        assert!(colored.contains("\x1b[31m/hooks/x\x1b[0m"));
    }

    #[test]
    fn relative_times() {
        let now = Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 0).unwrap();
        let at = |s: i64| now - chrono::Duration::seconds(s);
        assert_eq!(relative_time(at(2), now), "just now");
        assert_eq!(relative_time(at(-2), now), "just now");
        assert_eq!(relative_time(at(42), now), "42s ago");
        assert_eq!(relative_time(at(180), now), "3m ago");
        assert_eq!(relative_time(at(3 * 3600 + 5), now), "3h ago");
        assert_eq!(relative_time(at(2 * 86400), now), "2d ago");
        assert_eq!(relative_time(at(65 * 86400), now), "2mo ago");
        assert_eq!(relative_time(at(800 * 86400), now), "2y ago");
        assert_eq!(relative_time(at(-600), now), "in 10m");
        assert_eq!(ago(None), "-");
    }

    #[test]
    fn durations_and_sizes() {
        assert_eq!(human_duration(-3), "0s");
        assert_eq!(human_duration(42), "42s");
        assert_eq!(human_duration(192), "3m 12s");
        assert_eq!(human_duration(3900), "1h 05m");
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(12 * 1024 * 1024 + 300 * 1024), "12.3 MiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
        assert_eq!(span(None, None), "-");
    }

    #[test]
    fn limits() {
        assert_eq!(memory_limit_bytes(512 * 1024 * 1024), "512 MiB");
        assert_eq!(memory_limit_bytes(2 * 1024 * 1024 * 1024), "2 GiB");
        assert_eq!(memory_limit_bytes(1_000_000_000), "953.7 MiB");
        assert_eq!(memory_limit(Some(1536), Some(512)), "1.5 GiB");
        assert_eq!(memory_limit(None, Some(512)), "server default (512 MiB)");
        assert_eq!(memory_limit(Some(0), Some(0)), "unlimited (server default)");
        assert_eq!(memory_limit(None, None), "server default");
        assert_eq!(cpu_limit(Some(0.5), Some(1.0)), "0.5 CPU");
        assert_eq!(cpu_limit(None, Some(2.0)), "server default (2 CPUs)");
        assert_eq!(cpu_limit(None, Some(0.0)), "unlimited (server default)");
        assert_eq!(cpu_limit(None, None), "server default");
        assert_eq!((memory_or_unlimited(0), cpus_or_unlimited(0.0)), ("unlimited".into(), "unlimited".into()));
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("héllo wörld", 5), "héll…");
    }

    #[test]
    fn paint_respects_enabled_flag() {
        assert_eq!(paint(false, Color::Red, "x"), "x");
        assert_eq!(paint(true, Color::Red, "x"), "\x1b[31mx\x1b[0m");
        assert_eq!(paint(true, Color::Red, ""), "");
    }

    #[test]
    fn log_line_format() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 5).unwrap();
        let local = ts.with_timezone(&Local).format("%H:%M:%S").to_string();
        let line =
            LogLine { ts, stream: LogStreamKind::Stdout, instance: Some("a1b2c3".into()), line: "hello\n".into() };
        assert_eq!(format_log_line(&line, false), format!("{local} [a1b2c3] hello"));
        let sys = LogLine { ts, stream: LogStreamKind::System, instance: None, line: "==> Build successful".into() };
        assert_eq!(format_log_line(&sys, false), format!("{local} ==> Build successful"));
        let colored = format_log_line(&sys, true);
        assert!(colored.contains("\x1b[1;36m==> Build successful\x1b[0m"));
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
