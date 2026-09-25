//! Git sources via the `git` CLI: a persistent bare cache per service
//! (`<repos_dir>/<service_id>`), incremental fetches, branch / commit
//! resolution, and export of the tree (`git archive`) without `.git`.
//!
//! Git can never prompt (no terminal, no askpass, ssh in batch mode) and
//! credentials embedded in URLs never reach logs or error messages.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use ferry_core::git::redact_url;
use ferry_core::{CancellationToken, LogSink};
use tokio::process::Command;
use tokio_util::io::SyncIoBridge;

use crate::archive::{ExtractError, ExtractReport, LinkPolicy, extract_tar};
use crate::fsutil::remove_dir_async;
use crate::process::{RunError, run_capture, run_with_stdout};
use crate::redact::redact_text;

/// Upper bound for a fetch (stalled transfers are aborted much earlier by the
/// low-speed limit).
const FETCH_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(120);
const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);

const SSH_COMMAND: &str = "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=30";

/// Failure of a git operation. Messages are already redacted.
#[derive(Debug)]
pub(crate) enum GitError {
    Canceled,
    /// Invalid input or missing branch / commit.
    NotFound(String),
    Invalid(String),
    Failed(String),
}

impl GitError {
    pub(crate) fn message(&self) -> String {
        match self {
            GitError::Canceled => "canceled".to_string(),
            GitError::NotFound(m) | GitError::Invalid(m) | GitError::Failed(m) => m.clone(),
        }
    }
}

/// The result of checking out a git source.
#[derive(Debug, Clone)]
pub(crate) struct Checkout {
    pub sha: String,
    pub subject: String,
    pub report_skipped: Vec<String>,
    pub has_submodules: bool,
}

/// A repository location ready to hand to git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoUrl {
    /// What git is given (local relative paths made absolute).
    pub git_url: String,
    /// Local filesystem path, for existence checks.
    pub local_path: Option<PathBuf>,
    /// For logs and errors.
    pub display: String,
}

/// Classify and sanitize a repository URL: https/http/ssh/git URLs,
/// scp-style `user@host:path`, `file://` URLs and local paths.
pub(crate) fn prepare_repo_url(url: &str) -> Result<RepoUrl, GitError> {
    let u = url.trim();
    let display = redact_url(u);
    if u.is_empty() {
        return Err(GitError::Invalid("repository URL is empty".into()));
    }
    if u.starts_with('-') || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(GitError::Invalid(format!("invalid repository URL '{display}'")));
    }
    if let Some(idx) = u.find("://") {
        let scheme = u[..idx].to_ascii_lowercase();
        return match scheme.as_str() {
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git" => {
                Ok(RepoUrl { git_url: u.to_string(), local_path: None, display })
            }
            "file" => {
                let path = PathBuf::from(&u[idx + 3..]);
                Ok(RepoUrl { git_url: u.to_string(), local_path: Some(path), display })
            }
            _ => Err(GitError::Invalid(format!(
                "unsupported repository URL scheme '{scheme}' (use https, ssh, git, file or a local path)"
            ))),
        };
    }
    if is_scp_like(u) {
        return Ok(RepoUrl { git_url: u.to_string(), local_path: None, display });
    }
    let path = Path::new(u);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::path::absolute(path).map_err(|e| GitError::Invalid(format!("invalid repository path '{u}': {e}")))?
    };
    Ok(RepoUrl { git_url: abs.to_string_lossy().into_owned(), local_path: Some(abs), display })
}

/// `host:path` / `user@host:path` (git's scp-like syntax): a colon before
/// the first slash, and not a Windows drive letter.
fn is_scp_like(u: &str) -> bool {
    let Some(colon) = u.find(':') else { return false };
    let before = &u[..colon];
    let windows_drive = cfg!(windows) && before.len() == 1;
    !(before.is_empty() || before.contains('/') || windows_drive)
}

/// Branch names git would accept (subset of `git check-ref-format`), and
/// never anything that could be mistaken for an option.
pub(crate) fn validate_branch(b: &str) -> Result<(), GitError> {
    let bad = b.is_empty()
        || b.len() > 255
        || b.starts_with('-')
        || b.starts_with('/')
        || b.ends_with('/')
        || b.ends_with('.')
        || b.ends_with(".lock")
        || b.contains("..")
        || b.contains("//")
        || b.contains("@{")
        || b == "@"
        || b.split('/').any(|p| p.starts_with('.'))
        || b.chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    if bad { Err(GitError::Invalid(format!("invalid branch name '{b}'"))) } else { Ok(()) }
}

/// Commits are given as (possibly abbreviated) hex object names.
pub(crate) fn validate_commit(c: &str) -> Result<(), GitError> {
    if (4..=64).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(GitError::Invalid(format!("invalid commit '{c}': expected a hexadecimal commit sha")))
    }
}

/// A `git` command that can never prompt or hang on credentials.
fn git_cmd() -> Command {
    let mut c = Command::new("git");
    c.args([
        "-c",
        "credential.interactive=false",
        "-c",
        "gc.autoDetach=false",
        "-c",
        "maintenance.autoDetach=false",
        "-c",
        "log.showSignature=false",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]);
    c.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", SSH_COMMAND)
        .env("GIT_ALLOW_PROTOCOL", "file:git:http:https:ssh")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C");
    for var in [
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_SSH",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
    ] {
        c.env_remove(var);
    }
    c.current_dir(std::env::temp_dir());
    c
}

fn git_dir_arg(cache: &Path) -> OsString {
    let mut s = OsString::from("--git-dir=");
    s.push(cache.as_os_str());
    s
}

async fn run_git(
    cmd: Command,
    what: &str,
    url: &RepoUrl,
    cancel: Option<&CancellationToken>,
    timeout: Duration,
) -> Result<Output, GitError> {
    match run_capture(cmd, cancel, Some(timeout)).await {
        Ok(out) => Ok(out),
        Err(RunError::Canceled) => Err(GitError::Canceled),
        Err(RunError::Spawn(e)) if e.kind() == io::ErrorKind::NotFound => {
            Err(GitError::Failed("the git CLI was not found: install git on the Ferry server".into()))
        }
        Err(e) => Err(GitError::Failed(redact_text(&format!("{what} ({}) failed: {e}", url.display), &url.git_url))),
    }
}

/// The most meaningful line of git's stderr (`fatal:` / `error:`), redacted.
pub(crate) fn git_failure_reason(out: &Output, url: &str) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let pick = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("fatal:"))
        .or_else(|| lines.iter().rev().find(|l| l.starts_with("error:")))
        .or_else(|| lines.last());
    let reason = match pick {
        Some(l) => l.trim_start_matches("fatal:").trim_start_matches("error:").trim().to_string(),
        None => format!("git exited with {}", out.status),
    };
    let mut reason = redact_text(&reason, url);
    if reason.len() > 400 {
        let mut cut = 400;
        while !reason.is_char_boundary(cut) {
            cut -= 1;
        }
        reason.truncate(cut);
        reason.push('…');
    }
    reason
}

/// Resolve the sha a remote branch points to (`git ls-remote`).
pub(crate) async fn ls_remote_branch(repo_url: &str, branch: &str) -> Result<String, GitError> {
    let url = prepare_repo_url(repo_url)?;
    validate_branch(branch)?;
    check_local_exists(&url).await?;
    let refname = format!("refs/heads/{branch}");
    let mut cmd = git_cmd();
    cmd.args(["ls-remote", "--refs"]).arg(&url.git_url).arg(&refname);
    let out = run_git(cmd, "git ls-remote", &url, None, LS_REMOTE_TIMEOUT).await?;
    if !out.status.success() {
        return Err(GitError::Failed(format!(
            "cannot list branches of {}: {}",
            url.display,
            git_failure_reason(&out, &url.git_url)
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(_, name)| name.trim() == refname)
        .map(|(sha, _)| sha.trim().to_string())
        .ok_or_else(|| GitError::NotFound(format!("branch '{branch}' not found in {}", url.display)))
}

async fn check_local_exists(url: &RepoUrl) -> Result<(), GitError> {
    if let Some(p) = &url.local_path
        && tokio::fs::metadata(p).await.is_err()
    {
        return Err(GitError::NotFound(format!("repository path '{}' does not exist", p.display())));
    }
    Ok(())
}

/// Make sure `cache` is a bare repository whose `origin` is `url`
/// (recreating it otherwise). Returns true when it was (re)created.
async fn open_cache(cache: &Path, url: &RepoUrl, cancel: &CancellationToken) -> Result<bool, GitError> {
    if tokio::fs::metadata(cache).await.is_ok() {
        let mut cmd = git_cmd();
        cmd.arg(git_dir_arg(cache)).args(["config", "--get", "remote.origin.url"]);
        match run_git(cmd, "git config", url, Some(cancel), LOCAL_TIMEOUT).await {
            Ok(out) if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == url.git_url => {
                return Ok(false);
            }
            Err(GitError::Canceled) => return Err(GitError::Canceled),
            _ => {
                tracing::info!("git cache {} is stale or for another remote; recreating it", cache.display());
            }
        }
    }
    init_cache(cache, url, cancel).await?;
    Ok(true)
}

async fn init_cache(cache: &Path, url: &RepoUrl, cancel: &CancellationToken) -> Result<(), GitError> {
    remove_dir_async(cache.to_path_buf())
        .await
        .map_err(|e| GitError::Failed(format!("cannot reset git cache {}: {e}", cache.display())))?;
    if let Some(parent) = cache.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| GitError::Failed(format!("cannot create {}: {e}", parent.display())))?;
    }
    let mut cmd = git_cmd();
    cmd.args(["init", "--bare", "--quiet", "--template="]).arg(cache);
    let out = run_git(cmd, "git init", url, Some(cancel), LOCAL_TIMEOUT).await?;
    if !out.status.success() {
        return Err(GitError::Failed(format!(
            "cannot create git cache {}: {}",
            cache.display(),
            git_failure_reason(&out, &url.git_url)
        )));
    }
    let mut cmd = git_cmd();
    cmd.arg(git_dir_arg(cache)).args(["config", "remote.origin.url"]).arg(&url.git_url);
    let out = run_git(cmd, "git config", url, Some(cancel), LOCAL_TIMEOUT).await?;
    if !out.status.success() {
        let _ = remove_dir_async(cache.to_path_buf()).await;
        return Err(GitError::Failed(format!(
            "cannot configure git cache: {}",
            git_failure_reason(&out, &url.git_url)
        )));
    }
    Ok(())
}

/// `git fetch origin <refspecs>` into the cache.
async fn fetch(cache: &Path, url: &RepoUrl, refspecs: &[String], cancel: &CancellationToken) -> Result<(), GitError> {
    let mut cmd = git_cmd();
    cmd.arg(git_dir_arg(cache)).args(["fetch", "--quiet", "--no-tags", "--force", "origin"]).args(refspecs);
    let out = run_git(cmd, "git fetch", url, Some(cancel), FETCH_TIMEOUT).await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = git_failure_reason(&out, &url.git_url);
    if stderr.contains("couldn't find remote ref") || stderr.contains("not our ref") {
        Err(GitError::NotFound(reason))
    } else {
        Err(GitError::Failed(format!("git fetch from {} failed: {reason}", url.display)))
    }
}

/// Errors that suggest a damaged cache (worth one re-clone).
fn looks_corrupt(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    [
        "lock",
        "corrupt",
        "bad object",
        "not a git repository",
        "loose object",
        "packfile",
        "index-pack",
        "unable to read",
    ]
    .iter()
    .any(|p| m.contains(p))
}

async fn rev_parse(
    cache: &Path,
    url: &RepoUrl,
    rev: &str,
    cancel: &CancellationToken,
) -> Result<Option<String>, GitError> {
    let mut cmd = git_cmd();
    cmd.arg(git_dir_arg(cache)).args(["rev-parse", "--verify", "--quiet"]).arg(format!("{rev}^{{commit}}"));
    let out = run_git(cmd, "git rev-parse", url, Some(cancel), LOCAL_TIMEOUT).await?;
    if !out.status.success() {
        return Ok(None);
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let valid = (40..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit());
    Ok(valid.then_some(sha))
}

async fn commit_subject(
    cache: &Path,
    url: &RepoUrl,
    sha: &str,
    cancel: &CancellationToken,
) -> Result<String, GitError> {
    let mut cmd = git_cmd();
    cmd.arg(git_dir_arg(cache)).args(["show", "-s", "--format=%s"]).arg(sha);
    let out = run_git(cmd, "git show", url, Some(cancel), LOCAL_TIMEOUT).await?;
    if !out.status.success() {
        return Err(GitError::Failed(format!("cannot read commit {sha}: {}", git_failure_reason(&out, &url.git_url))));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_string())
}

/// Fetch `branch` into `refs/remotes/origin/<branch>`, recreating a damaged
/// cache once.
async fn fetch_branch(
    cache: &Path,
    url: &RepoUrl,
    branch: &str,
    fresh: bool,
    logs: &LogSink,
    cancel: &CancellationToken,
) -> Result<(), GitError> {
    let refspec = vec![format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")];
    match fetch(cache, url, &refspec, cancel).await {
        Err(GitError::Failed(msg)) if !fresh && looks_corrupt(&msg) => {
            logs.system("==> The git cache looks damaged, cloning again");
            init_cache(cache, url, cancel).await?;
            fetch(cache, url, &refspec, cancel).await
        }
        other => other,
    }
}

/// Fetch (incrementally, through the service's cache) and export the tree of
/// `branch` / `commit` into `dest`.
pub(crate) async fn checkout(
    cache: &Path,
    repo_url: &str,
    branch: &str,
    commit: Option<&str>,
    dest: &Path,
    logs: &LogSink,
    cancel: &CancellationToken,
) -> Result<Checkout, GitError> {
    let url = prepare_repo_url(repo_url)?;
    validate_branch(branch)?;
    let commit = commit.map(str::trim).filter(|c| !c.is_empty());
    if let Some(c) = commit {
        validate_commit(c)?;
    }
    match commit {
        Some(c) => logs.system(format!("==> Cloning from {} (branch {branch}, commit {c})", url.display)),
        None => logs.system(format!("==> Cloning from {} (branch {branch})", url.display)),
    }
    check_local_exists(&url).await?;

    let fresh = open_cache(cache, &url, cancel).await?;
    let sha = match commit {
        None => {
            match fetch_branch(cache, &url, branch, fresh, logs, cancel).await {
                Ok(()) => {}
                Err(GitError::NotFound(_)) => {
                    return Err(GitError::NotFound(format!("branch '{branch}' not found in {}", url.display)));
                }
                Err(e) => return Err(e),
            }
            rev_parse(cache, &url, &format!("refs/remotes/origin/{branch}"), cancel)
                .await?
                .ok_or_else(|| GitError::NotFound(format!("branch '{branch}' not found in {}", url.display)))?
        }
        Some(c) => resolve_commit(cache, &url, branch, c, fresh, logs, cancel).await?,
    };
    let subject = commit_subject(cache, &url, &sha, cancel).await?;
    let short = sha.get(..7).unwrap_or(&sha);
    if subject.is_empty() {
        logs.system(format!("==> Checked out {short}"));
    } else {
        logs.system(format!("==> Checked out {short}: {subject}"));
    }

    let report = export(cache, &url, &sha, dest, cancel).await?;
    let has_submodules = tokio::fs::metadata(dest.join(".gitmodules")).await.is_ok();
    Ok(Checkout { sha, subject, report_skipped: report.skipped, has_submodules })
}

async fn resolve_commit(
    cache: &Path,
    url: &RepoUrl,
    branch: &str,
    commit: &str,
    fresh: bool,
    logs: &LogSink,
    cancel: &CancellationToken,
) -> Result<String, GitError> {
    // Already known locally (e.g. a rebuild of a previous commit): no network.
    if !fresh && let Some(sha) = rev_parse(cache, url, commit, cancel).await? {
        return Ok(sha);
    }
    // Most commits are on the service's branch. A failure here is not final:
    // the broader fetch below reports connectivity problems.
    match fetch_branch(cache, url, branch, fresh, logs, cancel).await {
        Err(GitError::Canceled) => return Err(GitError::Canceled),
        Err(GitError::Invalid(m)) => return Err(GitError::Invalid(m)),
        _ => {}
    }
    if let Some(sha) = rev_parse(cache, url, commit, cancel).await? {
        return Ok(sha);
    }
    // Then every branch and tag.
    let all = vec!["+refs/heads/*:refs/remotes/origin/*".to_string(), "+refs/tags/*:refs/tags/*".to_string()];
    let last_failure = match fetch(cache, url, &all, cancel).await {
        Ok(()) | Err(GitError::NotFound(_)) => None,
        Err(GitError::Failed(m)) => Some(m),
        Err(e) => return Err(e),
    };
    if let Some(sha) = rev_parse(cache, url, commit, cancel).await? {
        return Ok(sha);
    }
    // Finally ask for the object directly (servers allowing reachable-sha fetches).
    if commit.len() == 40 || commit.len() == 64 {
        match fetch(cache, url, &[commit.to_string()], cancel).await {
            Err(GitError::Canceled) => return Err(GitError::Canceled),
            _ => {
                if let Some(sha) = rev_parse(cache, url, commit, cancel).await? {
                    return Ok(sha);
                }
            }
        }
    }
    match last_failure {
        Some(m) => Err(GitError::Failed(m)),
        None => Err(GitError::NotFound(format!("commit '{commit}' not found in {}", url.display))),
    }
}

/// Attributes forced on every archived tree: the export must match a plain
/// checkout, whatever `export-ignore` / `export-subst` the repo declares.
const ARCHIVE_ATTRIBUTES: &str = "* -export-ignore -export-subst\n";

/// `git archive <sha>` streamed straight into the safe tar extractor.
async fn export(
    cache: &Path,
    url: &RepoUrl,
    sha: &str,
    dest: &Path,
    cancel: &CancellationToken,
) -> Result<ExtractReport, GitError> {
    // `$GIT_DIR/info/attributes` takes precedence over in-tree .gitattributes.
    let info = cache.join("info");
    tokio::fs::create_dir_all(&info)
        .await
        .map_err(|e| GitError::Failed(format!("cannot prepare git cache {}: {e}", cache.display())))?;
    tokio::fs::write(info.join("attributes"), ARCHIVE_ATTRIBUTES)
        .await
        .map_err(|e| GitError::Failed(format!("cannot prepare git cache {}: {e}", cache.display())))?;
    let mut cmd = git_cmd();
    cmd.arg(git_dir_arg(cache)).args(["archive", "--format=tar"]).arg(sha);
    let dest = dest.to_path_buf();
    let token = cancel.clone();
    let res = run_with_stdout(cmd, cancel, move |stdout| async move {
        let bridge = SyncIoBridge::new(stdout);
        match tokio::task::spawn_blocking(move || extract_tar(bridge, &dest, LinkPolicy::Skip, &token)).await {
            Ok(r) => r,
            Err(e) => Err(ExtractError::Io(io::Error::other(format!("extraction task failed: {e}")))),
        }
    })
    .await;
    match res {
        Err(RunError::Canceled) => Err(GitError::Canceled),
        Err(e) => Err(GitError::Failed(format!("git archive failed: {e}"))),
        Ok((_, _, Err(ExtractError::Canceled))) => Err(GitError::Canceled),
        Ok((_, _, Err(e))) => Err(GitError::Failed(format!("exporting the source tree failed: {e}"))),
        Ok((status, stderr, Ok(report))) => {
            if status.success() {
                Ok(report)
            } else {
                let out = Output { status, stdout: Vec::new(), stderr };
                Err(GitError::Failed(format!("git archive failed: {}", git_failure_reason(&out, &url.git_url))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_urls() {
        let https = prepare_repo_url("https://u:tok@github.com/a/b.git").unwrap();
        assert_eq!(https.git_url, "https://u:tok@github.com/a/b.git");
        assert_eq!(https.display, "https://***@github.com/a/b.git");
        assert!(https.local_path.is_none());
        assert!(prepare_repo_url("git@github.com:a/b.git").unwrap().local_path.is_none());
        assert!(prepare_repo_url("ssh://git@github.com:22/a/b").unwrap().local_path.is_none());
        let file = prepare_repo_url("file:///srv/repo").unwrap();
        assert_eq!(file.local_path.as_deref(), Some(Path::new("/srv/repo")));
        let abs = prepare_repo_url("/srv/repo").unwrap();
        assert_eq!(abs.git_url, "/srv/repo");
        let rel = prepare_repo_url("some/dir").unwrap();
        assert!(Path::new(&rel.git_url).is_absolute());
        assert!(prepare_repo_url("").is_err());
        assert!(prepare_repo_url("--upload-pack=evil").is_err());
        assert!(prepare_repo_url("https://github.com/a b").is_err());
        assert!(matches!(prepare_repo_url("ext::sh -c x"), Err(GitError::Invalid(_))));
        assert!(prepare_repo_url("ftp://example.com/x").is_err());
    }

    #[test]
    fn invalid_url_errors_are_redacted() {
        let err = prepare_repo_url("https://u:supersecret@host/a b").unwrap_err().message();
        assert!(!err.contains("supersecret"), "{err}");
    }

    #[test]
    fn validates_branches_and_commits() {
        for ok in ["main", "feature/x", "release-1.2", "v1"] {
            assert!(validate_branch(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "a..b", "a b", "a~1", "x.lock", "a/", "/a", ".hidden", "a:b", "@", "a@{1}"] {
            assert!(validate_branch(bad).is_err(), "{bad}");
        }
        assert!(validate_commit("abc1234").is_ok());
        assert!(validate_commit(&"a".repeat(40)).is_ok());
        assert!(validate_commit("abc").is_err());
        assert!(validate_commit("HEAD").is_err());
        assert!(validate_commit("--all").is_err());
    }

    #[test]
    fn failure_reason_picks_fatal_and_redacts() {
        let url = "https://user:hunter2hunter2@example.com/r.git";
        let out = Output {
            status: exit_status(128),
            stdout: Vec::new(),
            stderr: format!("warning: foo\nfatal: unable to access '{url}/': Could not resolve host\n").into_bytes(),
        };
        let r = git_failure_reason(&out, url);
        assert!(r.starts_with("unable to access"), "{r}");
        assert!(!r.contains("hunter2"), "{r}");
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    #[cfg(not(unix))]
    fn exit_status(code: u32) -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code)
    }

    // --- tests against real local repositories (need the git CLI only) ---

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=Ferry Test", "-c", "user.email=test@ferry.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repo on `main` with two commits; returns (dir, first sha, second sha).
    fn sample_repo() -> (tempfile::TempDir, String, String) {
        let d = tempfile::tempdir().unwrap();
        git_in(d.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(d.path().join("app.txt"), "v1").unwrap();
        git_in(d.path(), &["add", "-A"]);
        git_in(d.path(), &["commit", "-q", "-m", "first commit"]);
        let first = git_in(d.path(), &["rev-parse", "HEAD"]);
        std::fs::write(d.path().join("app.txt"), "v2").unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/nested.txt"), "n").unwrap();
        git_in(d.path(), &["add", "-A"]);
        git_in(d.path(), &["commit", "-q", "-m", "second commit\n\nbody"]);
        let second = git_in(d.path(), &["rev-parse", "HEAD"]);
        (d, first, second)
    }

    fn lines(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ferry_core::LogLine>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(l) = rx.try_recv() {
            out.push(l.line);
        }
        out
    }

    #[tokio::test]
    async fn checks_out_branch_head_and_older_commit() {
        let (repo, first, second) = sample_repo();
        let work = tempfile::tempdir().unwrap();
        let cache = work.path().join("repos/srv-1");
        let url = repo.path().to_string_lossy().into_owned();
        let (logs, mut rx) = LogSink::channel();
        let cancel = CancellationToken::new();

        let dest = work.path().join("b1");
        std::fs::create_dir_all(&dest).unwrap();
        let co = checkout(&cache, &url, "main", None, &dest, &logs, &cancel).await.unwrap();
        assert_eq!(co.sha, second);
        assert_eq!(co.subject, "second commit");
        assert_eq!(std::fs::read_to_string(dest.join("app.txt")).unwrap(), "v2");
        assert!(dest.join("sub/nested.txt").is_file());
        assert!(!dest.join(".git").exists());
        let l = lines(&mut rx);
        assert!(l.contains(&format!("==> Cloning from {url} (branch main)")), "{l:?}");
        assert!(l.contains(&format!("==> Checked out {}: second commit", &second[..7])), "{l:?}");

        // An exact older commit (abbreviated), served from the cache.
        let dest2 = work.path().join("b2");
        std::fs::create_dir_all(&dest2).unwrap();
        let co = checkout(&cache, &url, "main", Some(&first[..10]), &dest2, &logs, &cancel).await.unwrap();
        assert_eq!(co.sha, first);
        assert_eq!(std::fs::read_to_string(dest2.join("app.txt")).unwrap(), "v1");
        assert!(!dest2.join("sub").exists());

        // New commits are fetched incrementally into the existing cache.
        std::fs::write(repo.path().join("app.txt"), "v3").unwrap();
        git_in(repo.path(), &["commit", "-q", "-am", "third"]);
        let third = git_in(repo.path(), &["rev-parse", "HEAD"]);
        let dest3 = work.path().join("b3");
        std::fs::create_dir_all(&dest3).unwrap();
        let co = checkout(&cache, &url, "main", None, &dest3, &logs, &cancel).await.unwrap();
        assert_eq!(co.sha, third);
        assert_eq!(std::fs::read_to_string(dest3.join("app.txt")).unwrap(), "v3");

        // A commit only reachable from another branch is found too.
        git_in(repo.path(), &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.path().join("feature.txt"), "f").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-q", "-m", "feature work"]);
        let feature = git_in(repo.path(), &["rev-parse", "HEAD"]);
        let dest4 = work.path().join("b4");
        std::fs::create_dir_all(&dest4).unwrap();
        let co = checkout(&cache, &url, "main", Some(&feature), &dest4, &logs, &cancel).await.unwrap();
        assert_eq!(co.sha, feature);
        assert!(dest4.join("feature.txt").is_file());

        // file:// URLs work as well.
        let dest5 = work.path().join("b5");
        std::fs::create_dir_all(&dest5).unwrap();
        let file_url = format!("file://{url}");
        let co = checkout(&cache, &file_url, "feature", None, &dest5, &logs, &cancel).await.unwrap();
        assert_eq!(co.sha, feature);
    }

    #[tokio::test]
    async fn export_matches_a_checkout_despite_gitattributes() {
        let (repo, _, _) = sample_repo();
        std::fs::write(repo.path().join(".gitattributes"), "Dockerfile export-ignore\nversion.txt export-subst\n")
            .unwrap();
        std::fs::write(repo.path().join("Dockerfile"), "FROM busybox").unwrap();
        std::fs::write(repo.path().join("version.txt"), "$Format:%H$").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-q", "-m", "attrs"]);
        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("b");
        std::fs::create_dir_all(&dest).unwrap();
        checkout(
            &work.path().join("c"),
            &repo.path().to_string_lossy(),
            "main",
            None,
            &dest,
            &LogSink::noop(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(dest.join("Dockerfile").is_file(), "export-ignore must not drop files");
        assert_eq!(std::fs::read_to_string(dest.join("version.txt")).unwrap(), "$Format:%H$");
    }

    #[tokio::test]
    async fn missing_branch_and_commit_are_clear_errors() {
        let (repo, _, _) = sample_repo();
        let work = tempfile::tempdir().unwrap();
        let cache = work.path().join("cache");
        let url = repo.path().to_string_lossy().into_owned();
        let logs = LogSink::noop();
        let cancel = CancellationToken::new();
        let dest = work.path().join("b");
        std::fs::create_dir_all(&dest).unwrap();

        let err = checkout(&cache, &url, "nope", None, &dest, &logs, &cancel).await.unwrap_err();
        assert!(matches!(&err, GitError::NotFound(m) if m == &format!("branch 'nope' not found in {url}")), "{err:?}");

        let err = checkout(&cache, &url, "main", Some("deadbeefdeadbeef"), &dest, &logs, &cancel).await.unwrap_err();
        assert!(
            matches!(&err, GitError::NotFound(m) if m.starts_with("commit 'deadbeefdeadbeef' not found")),
            "{err:?}"
        );

        let err = checkout(&cache, &url, "main", Some("HEAD~1"), &dest, &logs, &cancel).await.unwrap_err();
        assert!(matches!(err, GitError::Invalid(_)), "{err:?}");

        let missing = work.path().join("no-such-repo");
        let err = checkout(&cache, &missing.to_string_lossy(), "main", None, &dest, &logs, &cancel).await.unwrap_err();
        assert!(err.message().contains("does not exist"), "{err:?}");
    }

    #[tokio::test]
    async fn cache_is_recreated_when_the_remote_changes() {
        let (repo_a, _, a_head) = sample_repo();
        let (repo_b, _, _) = sample_repo();
        std::fs::write(repo_b.path().join("b-only.txt"), "b").unwrap();
        git_in(repo_b.path(), &["add", "-A"]);
        git_in(repo_b.path(), &["commit", "-q", "-m", "b"]);
        let b_head = git_in(repo_b.path(), &["rev-parse", "HEAD"]);
        let work = tempfile::tempdir().unwrap();
        let cache = work.path().join("cache");
        let logs = LogSink::noop();
        let cancel = CancellationToken::new();
        for (repo, head) in [(&repo_a, &a_head), (&repo_b, &b_head)] {
            let dest = tempfile::tempdir().unwrap();
            let co = checkout(&cache, &repo.path().to_string_lossy(), "main", None, dest.path(), &logs, &cancel)
                .await
                .unwrap();
            assert_eq!(&co.sha, head);
        }
        let origin = git_in(&cache, &["--git-dir=.", "config", "--get", "remote.origin.url"]);
        assert_eq!(origin, repo_b.path().to_string_lossy());
    }

    #[tokio::test]
    async fn credentials_never_leak_into_errors_or_logs() {
        let work = tempfile::tempdir().unwrap();
        let (logs, mut rx) = LogSink::channel();
        // Port 9 (discard) on loopback: connection refused, fast.
        let url = "https://deploy:supersecret-token@127.0.0.1:9/org/repo.git";
        let err = checkout(&work.path().join("c"), url, "main", None, work.path(), &logs, &CancellationToken::new())
            .await
            .unwrap_err();
        let msg = err.message();
        assert!(!msg.contains("supersecret"), "{msg}");
        assert!(msg.contains("https://***@127.0.0.1:9/org/repo.git"), "{msg}");
        for l in lines(&mut rx) {
            assert!(!l.contains("supersecret"), "{l}");
        }
        let err = ls_remote_branch(url, "main").await.unwrap_err();
        assert!(!err.message().contains("supersecret"), "{err:?}");
    }

    #[tokio::test]
    async fn ls_remote_resolves_branch_heads() {
        let (repo, _, second) = sample_repo();
        let url = repo.path().to_string_lossy().into_owned();
        assert_eq!(ls_remote_branch(&url, "main").await.unwrap(), second);
        let err = ls_remote_branch(&url, "missing").await.unwrap_err();
        assert!(matches!(&err, GitError::NotFound(m) if m.contains("branch 'missing' not found")), "{err:?}");
        assert!(matches!(ls_remote_branch(&url, "-x").await, Err(GitError::Invalid(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn escaping_symlinks_in_repos_are_skipped() {
        let (repo, _, _) = sample_repo();
        std::os::unix::fs::symlink("/etc/passwd", repo.path().join("evil")).unwrap();
        std::os::unix::fs::symlink("app.txt", repo.path().join("fine")).unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-q", "-m", "links"]);
        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("b");
        std::fs::create_dir_all(&dest).unwrap();
        let co = checkout(
            &work.path().join("c"),
            &repo.path().to_string_lossy(),
            "main",
            None,
            &dest,
            &LogSink::noop(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(co.report_skipped.len(), 1, "{:?}", co.report_skipped);
        assert!(std::fs::symlink_metadata(dest.join("evil")).is_err());
        assert_eq!(std::fs::read_to_string(dest.join("fine")).unwrap(), "v2");
    }

    #[tokio::test]
    async fn canceled_checkout_returns_canceled() {
        let (repo, _, _) = sample_repo();
        let work = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = checkout(
            &work.path().join("c"),
            &repo.path().to_string_lossy(),
            "main",
            None,
            work.path(),
            &LogSink::noop(),
            &cancel,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, GitError::Canceled), "{err:?}");
    }

    #[test]
    fn corrupt_detection() {
        assert!(looks_corrupt("cannot lock ref 'refs/remotes/origin/main'"));
        assert!(looks_corrupt("fatal: bad object HEAD"));
        assert!(!looks_corrupt("Could not resolve host: github.com"));
    }
}
