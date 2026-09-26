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
        let original = expr.trim();
        if original.is_empty() {
            return Err(Error::invalid("cron schedule is empty"));
        }
        // Normalize aliases croner doesn't know.
        let expr = match original.to_ascii_lowercase().as_str() {
            "@midnight" => "@daily".to_string(),
            _ => original.to_string(),
        };
        if expr.starts_with('@') {
            const ALIASES: &[&str] = &["@yearly", "@annually", "@monthly", "@weekly", "@daily", "@hourly"];
            if !ALIASES.contains(&expr.to_ascii_lowercase().as_str()) {
                return Err(Error::invalid(format!(
                    "invalid cron schedule '{original}': supported aliases are @yearly, @annually, @monthly, \
                     @weekly, @daily, @midnight and @hourly"
                )));
            }
        } else if expr.split_whitespace().count() != 5 {
            return Err(Error::invalid(format!(
                "invalid cron schedule '{original}': expected 5 fields (minute hour day-of-month month day-of-week), \
                 e.g. '*/15 * * * *'"
            )));
        }
        let cron = croner::Cron::from_str(&expr).map_err(|e| {
            Error::invalid(format!(
                "invalid cron schedule '{original}': {e} (fields: minute 0-59, hour 0-23, day-of-month 1-31, \
                 month 1-12, day-of-week 0-7)"
            ))
        })?;
        let schedule = Schedule { expr: original.to_string(), cron };
        // Reject schedules that can never fire (e.g. February 30th).
        if schedule.next_after(Utc::now()).is_none_or(|t| t > Utc::now() + chrono::Duration::days(366 * 5)) {
            return Err(Error::invalid(format!("cron schedule '{original}' never fires")));
        }
        Ok(schedule)
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
        assert!(Schedule::parse("@midnight").is_ok());
        assert!(Schedule::parse("@often").is_err());
        assert!(Schedule::parse("0 0 30 2 *").is_err());
    }
}
