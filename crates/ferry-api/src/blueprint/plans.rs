//! Render plans → container resource limits.
//!
//! Render sizes every resource with a `plan` (its instance type); Ferry turns
//! it into the memory / CPU limits of the containers it runs (the blueprint's
//! `memoryLimit` / `cpuLimit` keys override it, see [`super::LimitsSpec`]).
//!
//! * Services (web, private, worker, cron): Render's memory and CPU, except
//!   `free` and `starter`, which get **0.5 CPU** instead of Render's 0.1: on
//!   your own server a tenth of a core only makes a service slow (builds of
//!   interpreted apps, startup, health checks) and frees nothing up that the
//!   host would otherwise use.
//! * Postgres: the memory of the plan (`basic-256mb` → 256 MiB, `pro-4gb` →
//!   4 GiB, `accelerated-16gb` → 16 GiB; legacy `free` / `starter` → 256 MiB,
//!   `standard` → 1 GiB, `pro` → 4 GiB, `pro plus` → 8 GiB). The CPU is not
//!   set by the plan (server default).
//! * Key Value (Redis): the memory of the plan (`free` / `starter` → 256 MiB,
//!   `standard` → 1 GiB, `pro` → 5 GiB, `pro plus` → 10 GiB); CPU not set.
//! * Static sites have no plans on Render: their `plan` is ignored.
//!
//! Plan names are case-insensitive, and multi-word names accept any
//! separator: `pro plus`, `Pro_Plus`, `pro-plus` and `proplus` are the same.

use ferry_core::resources;

/// The limits a plan declares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanLimits {
    /// MiB.
    pub memory_mb: u32,
    /// CPUs; `None` = the plan doesn't set the CPU.
    pub cpus: Option<f64>,
}

/// The plans of one kind of resource.
#[derive(Debug, Clone, Copy)]
pub struct PlanTable {
    lookup: fn(&str) -> Option<PlanLimits>,
    /// The accepted plan names, for warnings.
    pub known: &'static str,
}

impl PlanTable {
    /// The limits of `plan`, `None` for an unknown plan.
    pub fn limits(&self, plan: &str) -> Option<PlanLimits> {
        (self.lookup)(plan)
    }
}

/// Web services, private services, background workers and cron jobs.
pub const SERVICES: PlanTable =
    PlanTable { lookup: service_plan, known: "free, starter, standard, pro, pro plus, pro max, pro ultra" };

/// Postgres databases.
pub const POSTGRES: PlanTable = PlanTable {
    lookup: postgres_plan,
    known: "basic-256mb, basic-1gb, basic-4gb, pro-<size>, accelerated-<size>, or the legacy free, starter, standard, pro, pro plus",
};

/// Key Value (Redis) instances.
pub const KEY_VALUE: PlanTable = PlanTable { lookup: key_value_plan, known: "free, starter, standard, pro, pro plus" };

const GIB: u32 = 1024;

/// Lowercase, without separators: `Pro Plus` / `pro_plus` / `pro-plus` → `proplus`.
fn compact(plan: &str) -> String {
    plan.chars().filter(|c| !matches!(c, ' ' | '\t' | '_' | '-')).flat_map(char::to_lowercase).collect()
}

fn service_plan(plan: &str) -> Option<PlanLimits> {
    let (memory_mb, cpus) = match compact(plan).as_str() {
        // Render: free 512 MB / 0.1 CPU, starter 512 MB / 0.5 CPU (see the module docs).
        "free" | "starter" => (512, 0.5),
        "standard" => (2 * GIB, 1.0),
        "pro" => (4 * GIB, 2.0),
        "proplus" => (8 * GIB, 4.0),
        "promax" => (16 * GIB, 4.0),
        "proultra" => (32 * GIB, 8.0),
        _ => return None,
    };
    Some(PlanLimits { memory_mb, cpus: Some(cpus) })
}

fn postgres_plan(plan: &str) -> Option<PlanLimits> {
    let legacy = match compact(plan).as_str() {
        "free" | "starter" => Some(256),
        "standard" => Some(GIB),
        "pro" => Some(4 * GIB),
        "proplus" => Some(8 * GIB),
        _ => None,
    };
    let memory_mb = legacy.or_else(|| sized_postgres_plan(plan))?;
    Some(PlanLimits { memory_mb, cpus: None })
}

/// `<tier>-<memory>` Postgres plans: `basic-256mb`, `pro-4gb`,
/// `accelerated-1024gb`. The memory needs a unit and must be a valid limit.
fn sized_postgres_plan(plan: &str) -> Option<u32> {
    let plan = plan.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    let (tier, size) = plan.rsplit_once('-')?;
    if !matches!(tier, "basic" | "pro" | "accelerated") || !["mb", "gb", "tb"].iter().any(|u| size.ends_with(u)) {
        return None;
    }
    let memory_mb = resources::parse_memory_mb(size).ok()?;
    resources::validate_memory_mb(memory_mb).ok()?;
    Some(memory_mb)
}

fn key_value_plan(plan: &str) -> Option<PlanLimits> {
    let memory_mb = match compact(plan).as_str() {
        "free" | "starter" => 256,
        "standard" => GIB,
        "pro" => 5 * GIB,
        "proplus" => 10 * GIB,
        _ => return None,
    };
    Some(PlanLimits { memory_mb, cpus: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(table: &PlanTable, plan: &str) -> Option<(u32, Option<f64>)> {
        table.limits(plan).map(|l| (l.memory_mb, l.cpus))
    }

    #[test]
    fn service_plans() {
        for (plan, want) in [
            ("free", (512, 0.5)),
            ("starter", (512, 0.5)),
            ("standard", (2048, 1.0)),
            ("pro", (4096, 2.0)),
            ("pro plus", (8192, 4.0)),
            ("pro max", (16384, 4.0)),
            ("pro ultra", (32768, 8.0)),
        ] {
            assert_eq!(limits(&SERVICES, plan), Some((want.0, Some(want.1))), "{plan}");
        }
        // any case, any separator
        for plan in ["pro_plus", "pro-plus", "proplus", "Pro Plus", " PRO-PLUS "] {
            assert_eq!(limits(&SERVICES, plan), Some((8192, Some(4.0))), "{plan}");
        }
        assert_eq!(limits(&SERVICES, "Starter"), Some((512, Some(0.5))));
        for plan in ["", "xl", "pro mega", "basic-1gb", "standard plus"] {
            assert_eq!(limits(&SERVICES, plan), None, "{plan}");
        }
    }

    #[test]
    fn postgres_plans() {
        for (plan, mb) in [
            ("basic-256mb", 256),
            ("basic-1gb", 1024),
            ("basic-4gb", 4096),
            ("pro-4gb", 4096),
            ("pro-16gb", 16384),
            ("pro-512gb", 512 * 1024),
            ("accelerated-16gb", 16384),
            ("accelerated-1024gb", 1024 * 1024),
            ("Basic_1GB", 1024),
            ("pro 8gb", 8192),
            // legacy plans
            ("free", 256),
            ("starter", 256),
            ("standard", 1024),
            ("pro", 4096),
            ("pro plus", 8192),
            ("pro_plus", 8192),
        ] {
            assert_eq!(limits(&POSTGRES, plan), Some((mb, None)), "{plan}");
        }
        for plan in [
            "",
            "basic",
            "basic-1",         // no unit
            "basic-1xb",       // unknown unit
            "mega-4gb",        // unknown tier
            "basic-8mb",       // below the minimum limit
            "accelerated-2tb", // above the maximum limit
            "pro max",
        ] {
            assert_eq!(limits(&POSTGRES, plan), None, "{plan}");
        }
    }

    #[test]
    fn key_value_plans() {
        for (plan, mb) in [
            ("free", 256),
            ("starter", 256),
            ("standard", 1024),
            ("pro", 5120),
            ("pro plus", 10240),
            ("PRO-PLUS", 10240),
        ] {
            assert_eq!(limits(&KEY_VALUE, plan), Some((mb, None)), "{plan}");
        }
        for plan in ["", "basic-256mb", "pro max", "large"] {
            assert_eq!(limits(&KEY_VALUE, plan), None, "{plan}");
        }
    }

    #[test]
    fn every_plan_is_a_valid_limit() {
        let names = ["free", "starter", "standard", "pro", "pro plus", "pro max", "pro ultra"];
        for table in [SERVICES, POSTGRES, KEY_VALUE] {
            for plan in names {
                if let Some(l) = table.limits(plan) {
                    assert!(resources::validate(Some(l.memory_mb), l.cpus).is_ok(), "{plan}: {l:?}");
                }
            }
        }
    }
}
