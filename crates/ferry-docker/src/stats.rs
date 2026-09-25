//! CPU / memory figures computed like the `docker stats` CLI.

use bollard::models::{ContainerCpuStats, ContainerMemoryStats, ContainerStatsResponse};

use crate::ContainerStats;

/// CPU usage in percent of one CPU (100% = one full core), as the docker CLI
/// computes it: `cpu_delta / system_delta * online_cpus * 100`. Missing
/// samples or non-positive deltas give `0.0`.
pub(crate) fn cpu_percent(current: Option<&ContainerCpuStats>, previous: Option<&ContainerCpuStats>) -> f64 {
    let (Some(cur), Some(pre)) = (current, previous) else {
        return 0.0;
    };
    let total = |s: &ContainerCpuStats| s.cpu_usage.as_ref().and_then(|u| u.total_usage);
    let (Some(cur_total), Some(pre_total)) = (total(cur), total(pre)) else {
        return 0.0;
    };
    let (Some(cur_system), Some(pre_system)) = (cur.system_cpu_usage, pre.system_cpu_usage) else {
        return 0.0;
    };
    if cur_total <= pre_total || cur_system <= pre_system {
        return 0.0;
    }
    let cpu_delta = (cur_total - pre_total) as f64;
    let system_delta = (cur_system - pre_system) as f64;
    let online_cpus = cur
        .online_cpus
        .filter(|n| *n > 0)
        .map(f64::from)
        .or_else(|| {
            cur.cpu_usage
                .as_ref()
                .and_then(|u| u.percpu_usage.as_ref())
                .filter(|v| !v.is_empty())
                .map(|v| v.len() as f64)
        })
        .unwrap_or(1.0);
    let percent = cpu_delta / system_delta * online_cpus * 100.0;
    if percent.is_finite() { percent } else { 0.0 }
}

/// Memory usage without the page cache, as the docker CLI reports it:
/// `usage - total_inactive_file` (cgroup v1) / `usage - inactive_file`
/// (cgroup v2), falling back to `usage - cache`, then plain `usage`.
pub(crate) fn memory_usage(mem: Option<&ContainerMemoryStats>) -> u64 {
    let Some(mem) = mem else {
        return 0;
    };
    let usage = mem.usage.unwrap_or(0);
    let stats = mem.stats.as_ref();
    for key in ["total_inactive_file", "inactive_file", "cache"] {
        if let Some(v) = stats.and_then(|s| s.get(key)).copied()
            && v < usage
        {
            return usage - v;
        }
    }
    usage
}

pub(crate) fn from_response(resp: &ContainerStatsResponse) -> ContainerStats {
    let memory = resp.memory_stats.as_ref();
    ContainerStats {
        cpu_percent: cpu_percent(resp.cpu_stats.as_ref(), resp.precpu_stats.as_ref()),
        memory_bytes: memory_usage(memory),
        memory_limit_bytes: memory.and_then(|m| m.limit).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bollard::models::ContainerCpuUsage;

    use super::*;

    fn cpu(total: u64, system: u64, online: Option<u32>, percpu: Option<Vec<u64>>) -> ContainerCpuStats {
        ContainerCpuStats {
            cpu_usage: Some(ContainerCpuUsage { total_usage: Some(total), percpu_usage: percpu, ..Default::default() }),
            system_cpu_usage: Some(system),
            online_cpus: online,
            ..Default::default()
        }
    }

    #[test]
    fn cpu_like_docker_cli() {
        // 50ms of CPU over 1s of system time on 4 CPUs → 20%.
        let pre = cpu(1_000_000_000, 100_000_000_000, Some(4), None);
        let cur = cpu(1_050_000_000, 101_000_000_000, Some(4), None);
        assert!((cpu_percent(Some(&cur), Some(&pre)) - 20.0).abs() < 1e-9);
        // online_cpus missing → length of percpu_usage.
        let cur2 = cpu(1_050_000_000, 101_000_000_000, None, Some(vec![1, 2]));
        assert!((cpu_percent(Some(&cur2), Some(&pre)) - 10.0).abs() < 1e-9);
        // neither → 1 CPU.
        let cur3 = cpu(1_050_000_000, 101_000_000_000, Some(0), None);
        assert!((cpu_percent(Some(&cur3), Some(&pre)) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn cpu_guards() {
        let pre = cpu(10, 100, Some(2), None);
        assert_eq!(cpu_percent(None, Some(&pre)), 0.0);
        assert_eq!(cpu_percent(Some(&pre), None), 0.0);
        assert_eq!(cpu_percent(Some(&pre), Some(&pre)), 0.0, "zero deltas");
        assert_eq!(cpu_percent(Some(&cpu(5, 200, Some(2), None)), Some(&pre)), 0.0, "counter went backwards");
        assert_eq!(cpu_percent(Some(&cpu(20, 100, Some(2), None)), Some(&pre)), 0.0, "zero system delta");
        assert_eq!(cpu_percent(Some(&ContainerCpuStats::default()), Some(&pre)), 0.0, "missing data");
        // A stopped container: precpu is all zeros / missing.
        assert_eq!(cpu_percent(Some(&cpu(10, 100, Some(2), None)), Some(&ContainerCpuStats::default())), 0.0);
    }

    fn mem(usage: u64, stats: &[(&str, u64)]) -> ContainerMemoryStats {
        ContainerMemoryStats {
            usage: Some(usage),
            stats: Some(stats.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_, _>>()),
            limit: Some(1 << 30),
            ..Default::default()
        }
    }

    #[test]
    fn memory_like_docker_cli() {
        assert_eq!(memory_usage(Some(&mem(1000, &[("inactive_file", 300)]))), 700);
        assert_eq!(memory_usage(Some(&mem(1000, &[("total_inactive_file", 100), ("inactive_file", 300)]))), 900);
        assert_eq!(memory_usage(Some(&mem(1000, &[("cache", 400)]))), 600);
        assert_eq!(memory_usage(Some(&mem(1000, &[("inactive_file", 5000)]))), 1000, "never underflows");
        assert_eq!(memory_usage(Some(&mem(1000, &[]))), 1000);
        assert_eq!(memory_usage(Some(&ContainerMemoryStats::default())), 0);
        assert_eq!(memory_usage(None), 0);
    }

    #[test]
    fn full_response() {
        let resp = ContainerStatsResponse {
            cpu_stats: Some(cpu(2_000, 20_000, Some(1), None)),
            precpu_stats: Some(cpu(1_000, 10_000, Some(1), None)),
            memory_stats: Some(mem(1000, &[("inactive_file", 200)])),
            ..Default::default()
        };
        let s = from_response(&resp);
        assert!((s.cpu_percent - 10.0).abs() < 1e-9);
        assert_eq!(s.memory_bytes, 800);
        assert_eq!(s.memory_limit_bytes, 1 << 30);
        assert_eq!(from_response(&ContainerStatsResponse::default()), ContainerStats::default());
    }
}
