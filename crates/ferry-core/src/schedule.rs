//! Cron schedules (standard 5-field syntax, evaluated in UTC).

use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::{Error, Result};

/// A parsed cron expression such as `*/15 * * * *` or `@hourly`.
#[derive(Debug, Clone)]
pub struct Schedule {
    expr: String,
    cron: croner::Cron,
}

impl Schedule {
    /// Parse a 5-field cron expression (or an alias like `@daily`).
    /// 6-field (seconds) expressions are rejected: minute granularity only.
    pub fn parse(expr: &str) -> Result<Self> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Err(Error::invalid("cron schedule is empty"));
        }
        if !expr.starts_with('@') && expr.split_whitespace().count() != 5 {
            return Err(Error::invalid(format!(
                "cron schedule '{expr}' must have exactly 5 fields (minute hour day-of-month month day-of-week)"
            )));
        }
        let cron =
            croner::Cron::from_str(expr).map_err(|e| Error::invalid(format!("invalid cron schedule '{expr}': {e}")))?;
        Ok(Schedule { expr: expr.to_string(), cron })
    }

    pub fn expr(&self) -> &str {
        &self.expr
    }

    /// First fire time strictly after `after`.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.cron.find_next_occurrence(&after, false).ok()
    }

    /// True if the schedule fires in the half-open window `(from, to]`.
    pub fn fires_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        self.next_after(from).is_some_and(|t| t <= to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_and_computes_next() {
        let s = Schedule::parse("*/15 * * * *").unwrap();
        let t = Utc.with_ymd_and_hms(2026, 1, 1, 10, 7, 30).unwrap();
        assert_eq!(s.next_after(t).unwrap(), Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 0).unwrap());
        assert!(s.fires_between(t, Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 0).unwrap()));
        assert!(!s.fires_between(t, Utc.with_ymd_and_hms(2026, 1, 1, 10, 14, 59).unwrap()));
        assert!(Schedule::parse("@hourly").is_ok());
        assert!(Schedule::parse("* * * *").is_err());
        assert!(Schedule::parse("0 * * * * *").is_err());
        assert!(Schedule::parse("61 * * * *").is_err());
    }
}
