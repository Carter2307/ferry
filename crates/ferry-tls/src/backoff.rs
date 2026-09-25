//! Per-host retry backoff after failed certificate requests.

use std::time::Duration;

use tokio::time::Instant;

/// Delay after the first failure.
pub(crate) const BACKOFF_INITIAL: Duration = Duration::from_secs(5 * 60);
/// Longest delay between two attempts.
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(6 * 60 * 60);

/// Delay before the next attempt after `failures` consecutive failures:
/// 5 min, 10 min, 20 min, … capped at 6 h.
pub(crate) fn delay(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    BACKOFF_INITIAL.saturating_mul(1u32 << exponent).min(BACKOFF_MAX)
}

/// Retry state of one host.
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    pub failures: u32,
    pub next_attempt: Instant,
    pub last_error: String,
}

impl Backoff {
    /// Record one more failure at `now`; returns the delay until the next attempt.
    pub(crate) fn fail(entry: Option<&Backoff>, now: Instant, error: &str) -> (Backoff, Duration) {
        let failures = entry.map_or(0, |b| b.failures).saturating_add(1);
        let wait = delay(failures);
        (Backoff { failures, next_attempt: now + wait, last_error: error.to_string() }, wait)
    }

    /// Still waiting at `now`?
    pub(crate) fn blocks(&self, now: Instant) -> bool {
        self.next_attempt > now
    }

    /// Entries whose host hasn't been retried long after it became eligible
    /// (the host is gone) can be forgotten.
    pub(crate) fn stale(&self, now: Instant) -> bool {
        now > self.next_attempt + BACKOFF_MAX
    }
}

/// Human-friendly duration for log lines ("5m", "2h", "6h").
pub(crate) fn human(d: Duration) -> String {
    let secs = d.as_secs();
    let (hours, minutes) = (secs / 3600, (secs % 3600) / 60);
    match (hours, minutes) {
        (0, 0) => format!("{secs}s"),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_five_minutes_up_to_six_hours() {
        let mins: Vec<u64> = (1..=9).map(|n| delay(n).as_secs() / 60).collect();
        assert_eq!(mins, vec![5, 10, 20, 40, 80, 160, 320, 360, 360]);
        assert_eq!(delay(0), BACKOFF_INITIAL);
        assert_eq!(delay(u32::MAX), BACKOFF_MAX);
    }

    #[test]
    fn fail_accumulates() {
        let now = Instant::now();
        let (b, wait) = Backoff::fail(None, now, "boom");
        assert_eq!((b.failures, wait), (1, BACKOFF_INITIAL));
        assert!(b.blocks(now));
        assert!(!b.blocks(now + BACKOFF_INITIAL));
        let (b2, wait2) = Backoff::fail(Some(&b), now, "again");
        assert_eq!((b2.failures, wait2, b2.last_error.as_str()), (2, BACKOFF_INITIAL * 2, "again"));
        assert!(!b2.stale(now));
        assert!(b2.stale(now + BACKOFF_INITIAL * 2 + BACKOFF_MAX + Duration::from_secs(1)));
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(Duration::from_secs(300)), "5m");
        assert_eq!(human(Duration::from_secs(4800)), "1h20m");
        assert_eq!(human(Duration::from_secs(21600)), "6h");
        assert_eq!(human(Duration::from_secs(5)), "5s");
    }
}
