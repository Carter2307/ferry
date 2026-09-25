//! `docker build` via the docker CLI (BuildKit, plain progress), streamed
//! line by line into the deploy log.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::ExitStatus;

use ferry_core::{CancellationToken, Error, LogSink, Result};
use tokio::process::Command;

use crate::process::{RunError, run_streaming};

/// Lines of output kept to explain a failure.
const TAIL_LINES: usize = 200;

pub(crate) struct DockerBuild<'a> {
    pub docker_bin: &'a str,
    pub dockerfile: &'a Path,
    pub context: &'a Path,
    pub tag: &'a str,
    pub labels: &'a BTreeMap<String, String>,
    /// Already filtered to usable names; values are passed through the
    /// child's environment, never on the command line.
    pub build_args: &'a [(String, String)],
    pub no_cache: bool,
}

impl DockerBuild<'_> {
    fn command(&self) -> Command {
        let mut cmd = Command::new(self.docker_bin);
        cmd.arg("build").arg("--progress=plain").arg("-f").arg(self.dockerfile).arg("-t").arg(self.tag);
        for (k, v) in self.labels {
            cmd.arg("--label").arg(format!("{k}={v}"));
        }
        for (k, v) in self.build_args {
            // `--build-arg KEY` (no value): docker reads KEY from its environment.
            cmd.arg("--build-arg").arg(k);
            cmd.env(k, v);
        }
        if self.no_cache {
            cmd.arg("--no-cache");
        }
        cmd.arg(self.context);
        cmd.env("DOCKER_BUILDKIT", "1")
            .env("BUILDKIT_PROGRESS", "plain")
            .env("DOCKER_CLI_HINTS", "false")
            .env("NO_COLOR", "1");
        cmd
    }
}

/// Env keys whose values should never show up in build logs.
fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    ["SECRET", "TOKEN", "PASSWORD", "PASSWD", "PASS", "KEY", "PRIVATE", "CREDENTIAL", "AUTH", "DATABASE_URL", "DSN"]
        .iter()
        .any(|p| k.contains(p))
}

fn mask(line: &str, secrets: &[String]) -> String {
    let mut out = line.to_string();
    for s in secrets {
        if out.contains(s.as_str()) {
            out = out.replace(s.as_str(), "***");
        }
    }
    out
}

/// Run the build. Returns `Error::Build` with a concise reason on failure and
/// `Error::Canceled` when `cancel` fires (the docker process tree is killed).
pub(crate) async fn build(b: &DockerBuild<'_>, logs: &LogSink, cancel: &CancellationToken) -> Result<()> {
    let mut secrets: Vec<String> =
        b.build_args.iter().filter(|(k, v)| looks_secret(k) && v.trim().len() >= 6).map(|(_, v)| v.clone()).collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let out_logs = logs.clone();
    let err_logs = logs.clone();
    let (s1, s2) = (secrets.clone(), secrets.clone());
    let res = run_streaming(
        b.command(),
        cancel,
        TAIL_LINES,
        move |l| out_logs.stdout(mask(l, &s1)),
        move |l| err_logs.stderr(mask(l, &s2)),
    )
    .await;
    match res {
        Ok((status, _)) if status.success() => Ok(()),
        Ok((status, tail)) => {
            let tail: Vec<String> = tail.iter().map(|l| mask(l, &secrets)).collect();
            Err(Error::Build(failure_reason(&tail, status)))
        }
        Err(RunError::Canceled) => Err(Error::Canceled),
        Err(RunError::Spawn(e)) if e.kind() == io::ErrorKind::NotFound => Err(Error::Build(format!(
            "the docker CLI ('{}') was not found: install Docker on the Ferry server",
            b.docker_bin
        ))),
        Err(e) => Err(Error::Build(format!("could not run docker build: {e}"))),
    }
}

/// Pick the most useful line out of the end of a failed build's output.
pub(crate) fn failure_reason(tail: &[String], status: ExitStatus) -> String {
    let lines: Vec<&str> = tail.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    // BuildKit ends with "ERROR: failed to build: failed to solve: <reason>".
    let reason = lines
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("ERROR:").or_else(|| l.strip_prefix("ERROR ")))
        .map(|r| {
            let mut r = r.trim();
            loop {
                let before = r;
                for p in ["failed to build:", "failed to solve:", "failed to compute cache key:"] {
                    if let Some(rest) = r.strip_prefix(p) {
                        r = rest.trim();
                    }
                }
                if before == r {
                    break;
                }
            }
            r.to_string()
        })
        .or_else(|| lines.iter().rev().find(|l| l.to_ascii_lowercase().contains("error")).map(|l| l.to_string()))
        .or_else(|| lines.last().map(|l| l.to_string()))
        .unwrap_or_else(|| format!("docker build exited with {status}"));
    truncate(&reason, 500)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(unix)]
    #[test]
    fn picks_buildkit_error() {
        let tail: Vec<String> = [
            "#7 [3/3] RUN exit 3",
            "#7 ERROR: process \"/bin/sh -c exit 3\" did not complete successfully: exit code: 3",
            "------",
            " > [3/3] RUN exit 3:",
            "------",
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c exit 3\" did not complete successfully: exit code: 3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            failure_reason(&tail, status(1)),
            "process \"/bin/sh -c exit 3\" did not complete successfully: exit code: 3"
        );
        let other = vec!["something".to_string(), "Error response from daemon: boom".to_string(), "".to_string()];
        assert_eq!(failure_reason(&other, status(1)), "Error response from daemon: boom");
        assert!(failure_reason(&[], status(2)).contains("exited"));
        let long = vec![format!("ERROR: {}", "x".repeat(2000))];
        assert!(failure_reason(&long, status(1)).len() < 520);
    }

    #[test]
    fn command_keeps_secrets_out_of_argv() {
        let labels = BTreeMap::from([("ferry.managed".to_string(), "true".to_string())]);
        let args = vec![("API_TOKEN".to_string(), "s3cr3t-value".to_string())];
        let b = DockerBuild {
            docker_bin: "docker",
            dockerfile: Path::new("/ctx/Dockerfile.ferry"),
            context: Path::new("/ctx"),
            tag: "ferry/web:dep-1",
            labels: &labels,
            build_args: &args,
            no_cache: true,
        };
        let cmd = b.command();
        let std = cmd.as_std();
        let argv: Vec<String> = std.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            argv,
            vec![
                "build",
                "--progress=plain",
                "-f",
                "/ctx/Dockerfile.ferry",
                "-t",
                "ferry/web:dep-1",
                "--label",
                "ferry.managed=true",
                "--build-arg",
                "API_TOKEN",
                "--no-cache",
                "/ctx"
            ]
        );
        assert!(!argv.iter().any(|a| a.contains("s3cr3t")));
        let envs: Vec<_> = std.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "API_TOKEN" && v.is_some_and(|v| v == "s3cr3t-value")));
        assert!(envs.iter().any(|(k, _)| *k == "DOCKER_BUILDKIT"));
    }

    #[test]
    fn masks_secret_values() {
        assert!(looks_secret("STRIPE_API_KEY"));
        assert!(looks_secret("github_token"));
        assert!(!looks_secret("NODE_ENV"));
        assert_eq!(mask("token=abcdef123 ok", &["abcdef123".to_string()]), "token=*** ok");
    }
}
