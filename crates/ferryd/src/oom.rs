//! ferryd's OOM score (Linux): when the host runs out of memory, the kernel
//! kills containers before the server. See DESIGN.md §15.

/// Should ferryd write `wanted` to its `oom_score_adj` (currently
/// `current`)? Not for 0 ("leave unchanged"), and never to *raise* a lower
/// value set by the service manager (e.g. systemd's `OOMScoreAdjust=-900`).
#[cfg(any(target_os = "linux", test))]
fn should_set_oom_score_adj(current: Option<i32>, wanted: i32) -> bool {
    wanted != 0 && current != Some(wanted) && !(wanted < 0 && current.is_some_and(|c| c < wanted))
}

/// Make the kernel's OOM killer pick containers (and anything else) before
/// ferryd when the host runs out of memory. Best effort: lowering the score
/// needs root or CAP_SYS_RESOURCE. Processes ferryd spawns (git, the docker
/// CLI) are reset to 0 by ferry-build (a runaway git must not be spared);
/// containers never inherit it (dockerd starts them).
#[cfg(target_os = "linux")]
pub fn set_oom_score_adj(wanted: i32) {
    const PATH: &str = "/proc/self/oom_score_adj";
    let current = std::fs::read_to_string(PATH).ok().and_then(|s| s.trim().parse::<i32>().ok());
    if !should_set_oom_score_adj(current, wanted) {
        tracing::debug!(?current, wanted, "leaving oom_score_adj unchanged");
        return;
    }
    match std::fs::write(PATH, wanted.to_string()) {
        Ok(()) => tracing::debug!(wanted, "set oom_score_adj"),
        Err(e) => tracing::warn!(
            "could not set ferryd's OOM score adjustment to {wanted} ({e}): the kernel may kill ferryd instead of \
             a runaway container. Run ferryd as root, or set OOMScoreAdjust={wanted} in its systemd unit (and pass \
             --oom-score-adj 0 to silence this warning)."
        ),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn set_oom_score_adj(wanted: i32) {
    if wanted != 0 {
        tracing::debug!(wanted, "--oom-score-adj only applies on Linux");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oom_score_adj_decision() {
        assert!(should_set_oom_score_adj(Some(0), -500));
        assert!(should_set_oom_score_adj(None, -500));
        assert!(should_set_oom_score_adj(Some(-100), -500));
        assert!(!should_set_oom_score_adj(Some(-500), -500), "already set");
        assert!(!should_set_oom_score_adj(Some(-900), -500), "never raise systemd's OOMScoreAdjust");
        assert!(!should_set_oom_score_adj(Some(0), 0), "0 = leave unchanged");
        assert!(!should_set_oom_score_adj(Some(-900), 0));
        assert!(should_set_oom_score_adj(Some(0), 300), "an explicit positive value is applied");
    }
}
