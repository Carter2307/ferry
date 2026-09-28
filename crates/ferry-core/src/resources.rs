//! Container resource limits: the accepted ranges, the effective limits of a
//! service or datastore (its own values, else the server defaults), and the
//! parsing / formatting of human sizes shared by `ferryd` flags, the CLI and
//! blueprints.
//!
//! Units: memory in MiB (`512`, `"512M"`, `"1.5G"`), CPU in CPUs (`0.5` =
//! half a core; `"500m"` = 500 millicores = `0.5`). Like Docker, `M`/`MB`/
//! `MiB` all mean MiB and `G`/`GB`/`GiB` all mean GiB.

use crate::{Error, Result};

/// Smallest accepted memory limit (MiB). Docker's own minimum is 6 MiB.
pub const MIN_MEMORY_LIMIT_MB: u32 = 16;
/// Largest accepted memory limit (MiB): 1 TiB.
pub const MAX_MEMORY_LIMIT_MB: u32 = 1024 * 1024;
/// Smallest accepted CPU limit (CPUs).
pub const MIN_CPU_LIMIT: f64 = 0.01;
/// Largest accepted CPU limit (CPUs).
pub const MAX_CPU_LIMIT: f64 = 512.0;

/// The limits one container runs with. `None` = unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Limits {
    pub memory_mb: Option<u32>,
    pub cpus: Option<f64>,
}

impl Limits {
    /// Memory limit in bytes (Docker's `Memory`).
    pub fn memory_bytes(&self) -> Option<i64> {
        self.memory_mb.filter(|m| *m > 0).map(|m| i64::from(m) * 1024 * 1024)
    }

    /// CPU quota in units of 1e-9 CPUs (Docker's `NanoCpus`).
    pub fn nano_cpus(&self) -> Option<i64> {
        self.cpus.filter(|c| c.is_finite() && *c > 0.0).map(|c| (c * 1e9).round() as i64)
    }
}

/// Check an explicit memory limit (MiB).
pub fn validate_memory_mb(mb: u32) -> Result<()> {
    if (MIN_MEMORY_LIMIT_MB..=MAX_MEMORY_LIMIT_MB).contains(&mb) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "memory limit must be between {MIN_MEMORY_LIMIT_MB} MiB and {} (got {mb} MiB)",
            format_memory_mb(MAX_MEMORY_LIMIT_MB)
        )))
    }
}

/// Check an explicit CPU limit (CPUs).
pub fn validate_cpus(cpus: f64) -> Result<()> {
    if cpus.is_finite() && (MIN_CPU_LIMIT..=MAX_CPU_LIMIT).contains(&cpus) {
        Ok(())
    } else {
        Err(Error::invalid(format!("CPU limit must be between {MIN_CPU_LIMIT} and {MAX_CPU_LIMIT} CPUs (got {cpus})")))
    }
}

/// Check both explicit limits of a service or datastore (`None` = default).
pub fn validate(memory_mb: Option<u32>, cpus: Option<f64>) -> Result<()> {
    if let Some(m) = memory_mb {
        validate_memory_mb(m)?;
    }
    if let Some(c) = cpus {
        validate_cpus(c)?;
    }
    Ok(())
}

/// Round a CPU limit to Docker's CLI granularity (0.01 CPU).
pub fn round_cpus(cpus: f64) -> f64 {
    (cpus * 100.0).round() / 100.0
}

/// Parse a memory size into MiB: `512`, `512M`, `512MiB`, `1G`, `1.5GB`,
/// `2g`. A plain number is MiB. `0` parses to 0 (callers decide what it
/// means). Sizes are rounded to whole MiB.
pub fn parse_memory_mb(s: &str) -> Result<u32> {
    let t = s.trim().to_ascii_lowercase();
    let bad = || Error::invalid(format!("invalid memory size '{s}' (examples: 512M, 1G, 1.5G)"));
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let value: f64 = num.parse().map_err(|_| bad())?;
    let factor = match unit.trim() {
        "" | "m" | "mb" | "mib" => 1.0,
        "g" | "gb" | "gib" => 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0,
        "k" | "kb" | "kib" => 1.0 / 1024.0,
        _ => return Err(bad()),
    };
    let mb = (value * factor).round();
    if !mb.is_finite() || mb < 0.0 || mb > f64::from(u32::MAX) || (value > 0.0 && mb == 0.0) {
        return Err(bad());
    }
    Ok(mb as u32)
}

/// Human form of a MiB amount: `512 MiB`, `1 GiB`, `1.5 GiB`, `1152 MiB`.
/// GiB only for quarter-GiB multiples: exact at 2 decimals, so the text
/// parses back to the same amount and nothing is rounded (the web client's
/// `formatMemoryMb` must print the same).
pub fn format_memory_mb(mb: u32) -> String {
    if mb >= 1024 && mb.is_multiple_of(256) {
        let gib = f64::from(mb) / 1024.0;
        if gib.fract() == 0.0 { format!("{gib:.0} GiB") } else { format!("{} GiB", trim_float(gib)) }
    } else {
        format!("{mb} MiB")
    }
}

/// Parse a CPU amount: `0.5`, `2`, or millicores `500m`. `0` parses to 0.
/// The result is rounded to 0.01 CPU; a non-zero amount that rounds to 0
/// (`0.004`, `4m`) is an error, never a silent 0 (which callers read as
/// "default" or "unlimited").
pub fn parse_cpus(s: &str) -> Result<f64> {
    let t = s.trim().to_ascii_lowercase();
    let bad = || Error::invalid(format!("invalid CPU amount '{s}' (examples: 0.5, 2, 500m)"));
    let value = match t.strip_suffix('m') {
        Some(millis) => millis.trim().parse::<f64>().map_err(|_| bad())? / 1000.0,
        None => t.trim_end_matches("cpus").trim_end_matches("cpu").trim().parse::<f64>().map_err(|_| bad())?,
    };
    if !value.is_finite() || value < 0.0 {
        return Err(bad());
    }
    let cpus = round_cpus(value);
    if value > 0.0 && cpus == 0.0 {
        return Err(Error::invalid(format!(
            "CPU limit must be between {MIN_CPU_LIMIT} and {MAX_CPU_LIMIT} CPUs (got {})",
            s.trim()
        )));
    }
    Ok(cpus)
}

/// Human form of a CPU amount: `0.5 CPU`, `1 CPU`, `2 CPUs`.
pub fn format_cpus(cpus: f64) -> String {
    let n = trim_float(cpus);
    if cpus > 1.0 { format!("{n} CPUs") } else { format!("{n} CPU") }
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_sizes() {
        assert_eq!(parse_memory_mb("512").unwrap(), 512);
        assert_eq!(parse_memory_mb("512M").unwrap(), 512);
        assert_eq!(parse_memory_mb(" 512mib ").unwrap(), 512);
        assert_eq!(parse_memory_mb("1G").unwrap(), 1024);
        assert_eq!(parse_memory_mb("1.5GB").unwrap(), 1536);
        assert_eq!(parse_memory_mb("2gib").unwrap(), 2048);
        assert_eq!(parse_memory_mb("0").unwrap(), 0);
        for bad in ["", "abc", "-1G", "1X", "1.2.3G", "1k"] {
            assert!(parse_memory_mb(bad).is_err(), "{bad}");
        }
        assert_eq!(format_memory_mb(512), "512 MiB");
        assert_eq!(format_memory_mb(1024), "1 GiB");
        assert_eq!(format_memory_mb(1536), "1.5 GiB");
        assert_eq!(format_memory_mb(1500), "1500 MiB");
        // Same as the web client's formatMemoryMb: GiB only when exact at 2
        // decimals, so it parses back (1152 MiB is 1.125 GiB, not "1.12 GiB").
        for (mb, text) in
            [(1088, "1088 MiB"), (1152, "1152 MiB"), (1280, "1.25 GiB"), (1664, "1664 MiB"), (1792, "1.75 GiB")]
        {
            assert_eq!(format_memory_mb(mb), text);
            assert_eq!(parse_memory_mb(&format_memory_mb(mb)).unwrap(), mb);
        }
    }

    #[test]
    fn cpu_amounts() {
        assert_eq!(parse_cpus("0.5").unwrap(), 0.5);
        assert_eq!(parse_cpus("2").unwrap(), 2.0);
        assert_eq!(parse_cpus("500m").unwrap(), 0.5);
        assert_eq!(parse_cpus("1 cpu").unwrap(), 1.0);
        assert_eq!(parse_cpus("0.333").unwrap(), 0.33);
        assert_eq!(parse_cpus("0").unwrap(), 0.0);
        assert_eq!(parse_cpus("0m").unwrap(), 0.0);
        assert_eq!(parse_cpus("0.005").unwrap(), 0.01);
        for bad in ["", "x", "-1", "inf", "NaN"] {
            assert!(parse_cpus(bad).is_err(), "{bad}");
        }
        // Rounding must not turn a tiny amount into 0 ("default" / "unlimited").
        for tiny in ["0.004", "4m", "0.001 cpu"] {
            let err = parse_cpus(tiny).unwrap_err().to_string();
            assert!(err.contains("CPU limit must be between 0.01 and 512 CPUs"), "{tiny}: {err}");
        }
        assert_eq!(format_cpus(0.5), "0.5 CPU");
        assert_eq!(format_cpus(1.0), "1 CPU");
        assert_eq!(format_cpus(2.0), "2 CPUs");
    }

    #[test]
    fn ranges_and_conversions() {
        assert!(validate(Some(512), Some(0.5)).is_ok());
        assert!(validate(None, None).is_ok());
        assert!(validate(Some(8), None).is_err());
        assert!(validate(None, Some(0.0)).is_err());
        assert!(validate(None, Some(f64::NAN)).is_err());
        assert!(validate(None, Some(1000.0)).is_err());
        let l = Limits { memory_mb: Some(512), cpus: Some(0.5) };
        assert_eq!(l.memory_bytes(), Some(512 << 20));
        assert_eq!(l.nano_cpus(), Some(500_000_000));
        assert_eq!(Limits::default().memory_bytes(), None);
        assert_eq!(Limits::default().nano_cpus(), None);
    }
}
