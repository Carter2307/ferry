//! `docker build` via the docker CLI (BuildKit, plain progress), streamed
//! line by line into the deploy log.
//!
//! Build-time env values always reach the CLI through its environment, never
//! its command line. Every variable is passed as a BuildKit secret
//! (`--secret id=KEY,env=FERRY_BUILD_SECRET_<n>`): generated Dockerfiles
//! mount them into their install/build steps only, and user Dockerfiles can
//! do the same with `RUN --mount=type=secret,id=KEY,env=KEY …`. User
//! Dockerfiles also keep getting `--build-arg KEY`, so the ARGs they declare
//! still receive values (BuildKit records those in the image history: that
//! is the Dockerfile author's choice). Generated Dockerfiles never declare
//! ARGs for env vars; they get `FERRY_BUILD_ENV_DIGEST` instead.
//!
//! The CLI is pointed at the daemon Ferry runs containers on and the image is
//! always `--load`ed, so a docker context or buildx builder configured for
//! something else cannot make the image land elsewhere.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use ferry_core::{CancellationToken, Error, LogSink, Result};
use tokio::process::Command;

use crate::dockerfile::{ENV_DIGEST_ARG, RESERVED_PREFIX, secret_id};
use crate::process::{RunError, run_streaming};

/// Lines of output kept to explain a failure.
const TAIL_LINES: usize = 200;

pub(crate) struct DockerBuild<'a> {
    pub docker_bin: &'a str,
    pub dockerfile: &'a Path,
    pub context: &'a Path,
    pub tag: &'a str,
    pub labels: &'a BTreeMap<String, String>,
    /// Build-time variables, already filtered to usable names. Passed as
    /// BuildKit secrets through the child's environment.
    pub env: &'a [(String, String)],
    /// Also pass them as `--build-arg KEY` (user Dockerfiles).
    pub build_args: bool,
    /// Value of `FERRY_BUILD_ENV_DIGEST` (generated Dockerfiles with env).
    pub env_digest: Option<&'a str>,
    /// `DOCKER_HOST` for the CLI (see [`daemon_host`]).
    pub docker_host: Option<&'a str>,
    pub no_cache: bool,
}

impl DockerBuild<'_> {
    fn command(&self) -> Command {
        let mut cmd = Command::new(self.docker_bin);
        cmd.arg("build").arg("--progress=plain").arg("--load").arg("-f").arg(self.dockerfile).arg("-t").arg(self.tag);
        for (k, v) in self.labels {
            cmd.arg("--label").arg(format!("{k}={v}"));
        }
        for (i, (k, v)) in self.env.iter().enumerate() {
            // The secret's source is a Ferry-owned variable of the CLI's
            // environment, so user vars (GODEBUG, HTTPS_PROXY...) never
            // configure the docker CLI itself on generated builds.
            let source = format!("{RESERVED_PREFIX}SECRET_{i}");
            cmd.arg("--secret").arg(format!("id={},env={source}", secret_id(k)));
            cmd.env(&source, v);
            if self.build_args {
                // `--build-arg KEY` (no value): docker reads KEY from its environment.
                cmd.arg("--build-arg").arg(k);
                cmd.env(k, v);
            }
        }
        if let Some(digest) = self.env_digest {
            cmd.arg("--build-arg").arg(format!("{ENV_DIGEST_ARG}={digest}"));
        }
        if self.no_cache {
            cmd.arg("--no-cache");
        }
        cmd.arg(self.context);
        cmd.env("DOCKER_BUILDKIT", "1")
            .env("BUILDKIT_PROGRESS", "plain")
            .env("DOCKER_CLI_HINTS", "false")
            .env("NO_COLOR", "1");
        if let Some(host) = self.docker_host {
            // DOCKER_HOST wins over any docker context.
            cmd.env("DOCKER_HOST", host).env_remove("DOCKER_CONTEXT");
        }
        cmd
    }
}

/// Unix sockets ferry-docker tries, in order, when `DOCKER_HOST` is unset.
/// `None` when `DOCKER_HOST` is set: the CLI reads it like ferry-docker does.
fn daemon_socket_candidates(docker_host: Option<&str>, home: Option<&str>) -> Option<Vec<PathBuf>> {
    if docker_host.is_some_and(|h| !h.trim().is_empty()) {
        return None;
    }
    let mut out = vec![PathBuf::from("/var/run/docker.sock")];
    if cfg!(target_os = "macos")
        && let Some(home) = home.map(str::trim).filter(|h| !h.is_empty())
    {
        out.push(Path::new(home).join(".docker/run/docker.sock"));
    }
    Some(out)
}

/// The daemon the docker CLI must build on: the one ferry-docker connects
/// to (`DOCKER_HOST`, else the first reachable default socket). Without it
/// the CLI would follow its current docker context, which may be another
/// daemon (Colima, OrbStack, a remote host) than the one running Ferry's
/// containers. `None`: leave the CLI's environment alone.
pub(crate) async fn daemon_host() -> Option<String> {
    let docker_host = std::env::var("DOCKER_HOST").ok();
    let home = std::env::var("HOME").ok();
    let candidates = daemon_socket_candidates(docker_host.as_deref(), home.as_deref())?;
    for path in candidates {
        if socket_reachable(&path).await {
            return Some(format!("unix://{}", path.display()));
        }
    }
    None
}

#[cfg(unix)]
async fn socket_reachable(path: &Path) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(2), tokio::net::UnixStream::connect(path)).await, Ok(Ok(_)))
}

#[cfg(not(unix))]
async fn socket_reachable(_path: &Path) -> bool {
    let _ = Duration::ZERO;
    false
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
        b.env.iter().filter(|(k, v)| looks_secret(k) && v.trim().len() >= 6).map(|(_, v)| v.clone()).collect();
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
    if let Some(step) = failed_step_reason(&lines, &reason) {
        return truncate(&step, 500);
    }
    if reason.contains("unexpected key 'env'") {
        return format!(
            "{} (secret env mounts need Docker Engine 27.3 or newer: upgrade Docker on the Ferry server)",
            truncate(&reason, 300)
        );
    }
    truncate(&reason, 500)
}

/// `process "<shell command>" did not complete successfully: exit code: N`
/// is BuildKit's reason for any failed `RUN`: useless as a summary (it is the
/// whole command). Explain it with the step's own output instead (see
/// [`step_output`]).
///
/// A package manager's error (npm, pnpm, yarn: see [`crate::pm_errors`])
/// wins; then the last `error:`-style line (its prefix removed); otherwise
/// the step and its exit code, plus its last output line.
fn failed_step_reason(lines: &[&str], reason: &str) -> Option<String> {
    if !(reason.starts_with("process \"") && reason.contains("did not complete successfully")) {
        return None;
    }
    let code = reason.rsplit_once("exit code:").map(|(_, c)| c.trim().to_string());
    let header = lines.iter().rposition(|l| l.starts_with("> [") && l.ends_with(':'));
    let output = step_output(lines, header);
    if let Some(summary) = crate::pm_errors::summarize(&output) {
        return Some(summary);
    }
    let explicit = output.iter().rev().find_map(|l| error_message(l, true));
    if let Some(line) = explicit.or_else(|| output.iter().rev().find_map(|l| error_message(l, false))) {
        return Some(line);
    }
    let step = step_command(lines[header?]);
    let failed = match &code {
        Some(c) => format!("{step} failed with exit code {c}"),
        None => format!("{step} failed"),
    };
    Some(match output.iter().rev().find(|l| !crate::pm_errors::is_noise(l)) {
        Some(last) => format!("{failed}: {last}"),
        None => failed,
    })
}

/// The failed step's output, timestamps removed. BuildKit streams every line
/// of it (`#8 0.763 npm error code E404`) and repeats only the last few in
/// its failure summary:
///
/// ```text
/// ------
///  > [build 4/4] RUN npm run build:
/// 0.107 error: publish directory 'nope' not found after the build
/// ------
/// ```
///
/// The streamed lines are used when the tail still holds them (npm's useful
/// line is often not among the last ones), else the summary block.
fn step_output<'a>(lines: &[&'a str], header: Option<usize>) -> Vec<&'a str> {
    if let Some(vertex) = failed_vertex(lines) {
        let streamed: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix(vertex).and_then(|rest| rest.strip_prefix(' ')))
            .filter_map(timestamped)
            .filter(|l| !l.is_empty())
            .collect();
        if !streamed.is_empty() {
            return streamed;
        }
    }
    let Some(header) = header else { return Vec::new() };
    lines[header + 1..]
        .iter()
        .take_while(|l| !l.starts_with("------"))
        .map(|l| strip_timestamp(l))
        .filter(|l| !l.is_empty())
        .collect()
}

/// `#8` out of `#8 ERROR: process "…" did not complete successfully: …`.
fn failed_vertex<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().rev().find_map(|l| {
        let (id, rest) = l.split_once(' ')?;
        let numbered = id.len() > 1 && id.starts_with('#') && id[1..].chars().all(|c| c.is_ascii_digit());
        (numbered && rest.starts_with("ERROR:")).then_some(id)
    })
}

/// `0.763 npm error …` → `npm error …`; `None` for BuildKit's own lines of
/// the vertex (`[build 4/4] RUN …`, `DONE 0.5s`, `ERROR: …`).
fn timestamped(rest: &str) -> Option<&str> {
    let (t, msg) = rest.split_once(' ').unwrap_or((rest, ""));
    let is_time = t.contains('.') && t.chars().all(|c| c.is_ascii_digit() || c == '.');
    is_time.then(|| msg.trim())
}

/// Output lines of the summary block start with the step's elapsed time.
fn strip_timestamp(line: &str) -> &str {
    let line = line.trim();
    match line.split_once(' ') {
        Some((t, rest)) if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.') => rest.trim(),
        _ if line.chars().all(|c| c.is_ascii_digit() || c == '.') => "",
        _ => line,
    }
}

/// The message of an error line, minus its prefix. `explicit`: `error: …`,
/// `ERROR: …`, `fatal: …` (what Ferry's own steps, pip, cargo and git
/// print); otherwise tool-specific forms (`npm error …`, `error[E0425]: …`).
/// Pointers to other logs are not an explanation.
fn error_message(line: &str, explicit: bool) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let msg = if explicit {
        ["error:", "fatal:"].iter().find(|p| lower.starts_with(**p)).map(|p| &line[p.len()..])
    } else if crate::pm_errors::is_noise(line) {
        None
    } else if let Some(p) = ["npm error ", "npm err! "].iter().find(|p| lower.starts_with(**p)) {
        Some(&line[p.len()..])
    } else if lower.starts_with("error[") {
        line.split_once("]:").map(|(_, m)| m)
    } else {
        None
    };
    let msg = msg?.trim();
    // "npm error 404" alone says nothing.
    (msg.len() >= 8).then(|| msg.to_string())
}

/// `> [build 4/4] RUN --mount=… npm run build:` → `RUN npm run build`
/// (flags dropped, shortened).
fn step_command(header: &str) -> String {
    let body = header.trim_end_matches(':');
    let body = body.split_once("] ").map_or(body, |(_, rest)| rest);
    let words: Vec<&str> = body
        .split_whitespace()
        .filter(|w| !["--mount=", "--network=", "--security="].iter().any(|f| w.starts_with(f)))
        .collect();
    truncate(&words.join(" "), 80)
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
        // A failed RUN without output: the step and its exit code, not the
        // raw `process "/bin/sh -c …"` string.
        assert_eq!(failure_reason(&tail, status(1)), "RUN exit 3 failed with exit code 3");
        let other = vec!["something".to_string(), "Error response from daemon: boom".to_string(), "".to_string()];
        assert_eq!(failure_reason(&other, status(1)), "Error response from daemon: boom");
        assert!(failure_reason(&[], status(2)).contains("exited"));
        let long = vec![format!("ERROR: {}", "x".repeat(2000))];
        assert!(failure_reason(&long, status(1)).len() < 520);
    }

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.as_std().get_envs().find(|(k, _)| *k == key).and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn command_keeps_secrets_out_of_argv() {
        let labels = BTreeMap::from([("ferry.managed".to_string(), "true".to_string())]);
        let env = vec![("API_TOKEN".to_string(), "s3cr3t-value".to_string())];
        // Generated Dockerfile: secrets + digest, no build args.
        let b = DockerBuild {
            docker_bin: "docker",
            dockerfile: Path::new("/ctx/Dockerfile.ferry"),
            context: Path::new("/ctx"),
            tag: "ferry/web:dep-1",
            labels: &labels,
            env: &env,
            build_args: false,
            env_digest: Some("0123abcd"),
            docker_host: Some("unix:///var/run/docker.sock"),
            no_cache: true,
        };
        let cmd = b.command();
        assert_eq!(
            argv(&cmd),
            vec![
                "build",
                "--progress=plain",
                "--load",
                "-f",
                "/ctx/Dockerfile.ferry",
                "-t",
                "ferry/web:dep-1",
                "--label",
                "ferry.managed=true",
                "--secret",
                "id=API_TOKEN,env=FERRY_BUILD_SECRET_0",
                "--build-arg",
                "FERRY_BUILD_ENV_DIGEST=0123abcd",
                "--no-cache",
                "/ctx"
            ]
        );
        assert_eq!(env_of(&cmd, "FERRY_BUILD_SECRET_0").as_deref(), Some("s3cr3t-value"));
        // The user's variable never configures the docker CLI itself.
        assert_eq!(env_of(&cmd, "API_TOKEN"), None);
        assert_eq!(env_of(&cmd, "DOCKER_BUILDKIT").as_deref(), Some("1"));
        assert_eq!(env_of(&cmd, "DOCKER_HOST").as_deref(), Some("unix:///var/run/docker.sock"));
        assert!(cmd.as_std().get_envs().any(|(k, v)| k == "DOCKER_CONTEXT" && v.is_none()), "DOCKER_CONTEXT removed");

        // User Dockerfile: secrets and `--build-arg KEY` (value from the env).
        let b = DockerBuild { build_args: true, env_digest: None, docker_host: None, no_cache: false, ..b };
        let cmd = b.command();
        let args = argv(&cmd);
        assert!(args.windows(2).any(|w| w == ["--secret", "id=API_TOKEN,env=FERRY_BUILD_SECRET_0"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--build-arg", "API_TOKEN"]), "{args:?}");
        assert!(!args.iter().any(|a| a.contains("s3cr3t") || a.contains("FERRY_BUILD_ENV_DIGEST")), "{args:?}");
        assert_eq!(env_of(&cmd, "API_TOKEN").as_deref(), Some("s3cr3t-value"));
        assert_eq!(env_of(&cmd, "DOCKER_HOST"), None);
    }

    #[test]
    fn daemon_socket_candidates_follow_ferry_docker() {
        assert_eq!(daemon_socket_candidates(Some("tcp://10.0.0.1:2375"), Some("/Users/me")), None);
        let c = daemon_socket_candidates(None, Some("/Users/me")).unwrap();
        assert_eq!(c[0], PathBuf::from("/var/run/docker.sock"));
        if cfg!(target_os = "macos") {
            assert_eq!(
                c,
                vec![PathBuf::from("/var/run/docker.sock"), PathBuf::from("/Users/me/.docker/run/docker.sock")]
            );
        } else {
            assert_eq!(c.len(), 1);
        }
        assert_eq!(daemon_socket_candidates(Some("  "), None).unwrap(), vec![PathBuf::from("/var/run/docker.sock")]);
    }

    fn tail(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[cfg(unix)]
    #[test]
    fn failed_run_step_is_explained_by_its_output() {
        // What BuildKit prints when the generated publish step fails.
        let t = tail(&[
            "#7 [build 4/4] RUN mkdir -p /ferry-publish && if [ -d \"nope\" ]; then cp -a \"nope/.\" /ferry-publish/; else echo \"error: publish directory 'nope' not found after the build\" >&2; exit 1; fi",
            "#7 0.107 error: publish directory 'nope' not found after the build",
            "#7 ERROR: process \"/bin/sh -c mkdir -p /ferry-publish && if [ -d \\\"nope\\\" ]; then …; fi\" did not complete successfully: exit code: 1",
            "------",
            " > [build 4/4] RUN mkdir -p /ferry-publish && if [ -d \"nope\" ]; then cp -a \"nope/.\" /ferry-publish/; else echo \"error: publish directory 'nope' not found after the build\" >&2; exit 1; fi:",
            "0.107 error: publish directory 'nope' not found after the build",
            "------",
            "Dockerfile.ferry:12",
            "--------------------",
            "  12 | >>> RUN mkdir -p /ferry-publish && …",
            "--------------------",
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c mkdir -p /ferry-publish && if [ -d \\\"nope\\\" ]; then …; fi\" did not complete successfully: exit code: 1",
        ]);
        assert_eq!(failure_reason(&t, status(1)), "publish directory 'nope' not found after the build");

        // pip: the last ERROR line; npm: the last meaningful `npm error` line.
        let step = |header: &str, output: &[&str], code: u8| -> Vec<String> {
            let mut v = vec!["------".to_string(), format!(" > [3/4] {header}:")];
            v.extend(output.iter().map(|l| format!("1.234 {l}")));
            v.push("------".into());
            v.push(format!(
                "ERROR: failed to build: failed to solve: process \"/bin/sh -c x\" did not complete successfully: exit code: {code}"
            ));
            v
        };
        let t = step(
            "RUN --mount=type=secret,id=A,env=A pip install --no-cache-dir -r requirements.txt",
            &[
                "ERROR: Could not find a version that satisfies the requirement nope==9 (from versions: none)",
                "ERROR: No matching distribution found for nope==9",
                "",
                "[notice] A new release of pip is available",
            ],
            1,
        );
        assert_eq!(failure_reason(&t, status(1)), "No matching distribution found for nope==9");
        let t = step(
            "RUN npm ci",
            &[
                "npm error code E404",
                "npm error 404 Not Found - GET https://registry.npmjs.org/nope - Not found",
                "npm error 404",
                "npm error A complete log of this run can be found in: /root/.npm/_logs/x.log",
            ],
            1,
        );
        assert_eq!(
            failure_reason(&t, status(1)),
            "404 Not Found - GET https://registry.npmjs.org/nope - Not found (npm E404)"
        );
        // No error line: the step (without its flags), exit code and last line.
        let t = step(
            "RUN --mount=type=secret,id=API_KEY,env=API_KEY --mount=type=cache,target=/root/.cache/go-build ( make site )",
            &["building...", "make: *** [site] Killed"],
            2,
        );
        assert_eq!(failure_reason(&t, status(1)), "RUN ( make site ) failed with exit code 2: make: *** [site] Killed");
        let t = step("RUN exit 7", &[], 7);
        assert_eq!(failure_reason(&t, status(1)), "RUN exit 7 failed with exit code 7");
        // Other failures keep BuildKit's reason.
        let t = tail(&["ERROR: failed to build: failed to solve: nginx:nope: not found"]);
        assert_eq!(failure_reason(&t, status(1)), "nginx:nope: not found");
        // Old BuildKit without secret env mounts: say what to do.
        let t =
            tail(&["ERROR: failed to solve: dockerfile parse error on line 7: unexpected key 'env' in 'env=API_KEY'"]);
        assert!(failure_reason(&t, status(1)).contains("Docker Engine 27.3 or newer"));
    }

    #[cfg(unix)]
    #[test]
    fn npm_install_failure_names_the_missing_package() {
        // Verbatim tail of the regression build: BuildKit's summary block
        // only repeats npm's last lines (advice), the streamed lines of the
        // vertex hold the explanation.
        let t = tail(&[
            "#8 [4/5] RUN npm install --include=dev --no-audit --no-fund",
            "#8 0.763 npm error code E404",
            "#8 0.763 npm error 404 Not Found - GET https://registry.npmjs.org/ferry-this-package-does-not-exist-xyz - Not found",
            "#8 0.763 npm error 404",
            "#8 0.763 npm error 404  'ferry-this-package-does-not-exist-xyz@1.0.0' is not in this registry.",
            "#8 0.763 npm error 404",
            "#8 0.763 npm error 404 Note that you can also install from a",
            "#8 0.763 npm error 404 tarball, folder, http url, or git url.",
            "#8 0.765 npm notice",
            "#8 0.765 npm notice New major version of npm available! 10.9.9 -> 12.1.0",
            "#8 0.765 npm notice Changelog: https://github.com/npm/cli/releases/tag/v12.1.0",
            "#8 0.765 npm notice To update run: npm install -g npm@12.1.0",
            "#8 0.765 npm notice",
            "#8 0.766 npm error A complete log of this run can be found in: /root/.npm/_logs/2026-09-26T13_53_54_546Z-debug-0.log",
            "#8 ERROR: process \"/bin/sh -c npm install --include=dev --no-audit --no-fund\" did not complete successfully: exit code: 1",
            "------",
            " > [4/5] RUN npm install --include=dev --no-audit --no-fund:",
            "0.763 npm error 404  'ferry-this-package-does-not-exist-xyz@1.0.0' is not in this registry.",
            "0.763 npm error 404",
            "0.763 npm error 404 Note that you can also install from a",
            "0.763 npm error 404 tarball, folder, http url, or git url.",
            "0.765 npm notice",
            "0.765 npm notice New major version of npm available! 10.9.9 -> 12.1.0",
            "0.765 npm notice Changelog: https://github.com/npm/cli/releases/tag/v12.1.0",
            "0.765 npm notice To update run: npm install -g npm@12.1.0",
            "0.765 npm notice",
            "0.766 npm error A complete log of this run can be found in: /root/.npm/_logs/2026-09-26T13_53_54_546Z-debug-0.log",
            "------",
            "Dockerfile.ferry:7",
            "--------------------",
            "   5 |     COPY package.json ./",
            "   6 | >>> RUN npm install --include=dev --no-audit --no-fund",
            "--------------------",
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c npm install --include=dev --no-audit --no-fund\" did not complete successfully: exit code: 1",
        ]);
        assert_eq!(
            failure_reason(&t, status(1)),
            "'ferry-this-package-does-not-exist-xyz@1.0.0' is not in this registry (npm E404)"
        );
        // Only the summary block left in the tail: still not the advice.
        let summary_only: Vec<String> = t.iter().skip_while(|l| *l != "------").cloned().collect();
        assert_eq!(
            failure_reason(&summary_only, status(1)),
            "'ferry-this-package-does-not-exist-xyz@1.0.0' is not in this registry"
        );

        // The project's own npm script: the tool's output, not npm's
        // metadata lines, explains the failure.
        let t = tail(&[
            "#9 [build 5/5] RUN npm run build",
            "#9 0.4 > app@1.0.0 build",
            "#9 0.4 > tsc",
            "#9 2.1 src/main.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.",
            "#9 2.2 npm error Lifecycle script `build` failed with error:",
            "#9 2.2 npm error code 2",
            "#9 2.2 npm error path /app",
            "#9 2.2 npm error command failed",
            "#9 2.2 npm error command sh -c tsc",
            "#9 ERROR: process \"/bin/sh -c npm run build\" did not complete successfully: exit code: 2",
            "------",
            " > [build 5/5] RUN npm run build:",
            "2.2 npm error command sh -c tsc",
            "------",
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c npm run build\" did not complete successfully: exit code: 2",
        ]);
        assert_eq!(
            failure_reason(&t, status(1)),
            "RUN npm run build failed with exit code 2: src/main.ts(3,7): error TS2322: Type 'string' is not \
             assignable to type 'number'."
        );

        // Streamed output of other vertexes never leaks into the summary.
        let t = tail(&[
            "#5 0.1 npm error code E404",
            "#5 0.1 npm error 404  'other@1.0.0' is not in this registry.",
            "#5 DONE 0.2s",
            "#9 [build 3/3] RUN sh build.sh",
            "#9 0.2 building...",
            "#9 0.3 make: *** [site] Error 2",
            "#9 ERROR: process \"/bin/sh -c sh build.sh\" did not complete successfully: exit code: 2",
            "------",
            " > [build 3/3] RUN sh build.sh:",
            "0.3 make: *** [site] Error 2",
            "------",
            "ERROR: failed to build: failed to solve: process \"/bin/sh -c sh build.sh\" did not complete successfully: exit code: 2",
        ]);
        assert_eq!(failure_reason(&t, status(1)), "RUN sh build.sh failed with exit code 2: make: *** [site] Error 2");
    }

    #[test]
    fn masks_secret_values() {
        assert!(looks_secret("STRIPE_API_KEY"));
        assert!(looks_secret("github_token"));
        assert!(!looks_secret("NODE_ENV"));
        assert_eq!(mask("token=abcdef123 ok", &["abcdef123".to_string()]), "token=*** ok");
    }
}
