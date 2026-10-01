//! Git sources via the `git` CLI: a persistent bare cache per service
//! (`<repos_dir>/<service_id>`), incremental fetches, branch / commit
//! resolution, and export of the tree (`git archive`) without `.git`.
//!
//! Git can never prompt (no terminal, no askpass, ssh in batch mode) and
//! credentials — embedded in a URL, or given next to it (a connected
//! account's token) — never reach logs or error messages. They do not reach
//! git's command line or the cache's config either: the URL git is given has
//! its userinfo removed, and a credential helper reads the username and
//! password from the git process's environment (readable by the same user
//! only, unlike `ps` output or the world-readable cache directory). The
//! helper only answers for the host of the URL, so a redirect to another
//! host is never sent the credentials.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use ferry_core::git::redact_url;
use ferry_core::{CancellationToken, LogSink};
use tokio::process::Command;
use tokio_util::io::SyncIoBridge;

use crate::archive::{ExtractError, ExtractReport, Limits, LinkPolicy, extract_tar};
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

/// A git source to check out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Source<'a> {
    pub repo_url: &'a str,
    /// Credentials that are not part of the URL (see
    /// [`RepoUrl::authenticated`]).
    pub credentials: Option<&'a Credentials>,
    pub branch: &'a str,
    pub commit: Option<&'a str>,
}

/// A repository location ready to hand to git.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RepoUrl {
    /// What git is given: local relative paths made absolute, and never any
    /// credentials (see [`RepoUrl::credentials`]).
    pub git_url: String,
    /// Username/password of an http(s) remote: removed from the URL
    /// (percent-decoded), or given next to it.
    pub credentials: Option<Credentials>,
    /// `host[:port]` of an http(s) URL, as git names it to credential
    /// helpers: the only host [`RepoUrl::credentials`] are for.
    pub credential_host: Option<String>,
    /// The URL as configured, only used to redact anything echoing it.
    pub original: String,
    /// Local filesystem path, for existence checks.
    pub local_path: Option<PathBuf>,
    /// For logs and errors.
    pub display: String,
}

impl RepoUrl {
    /// Use `credentials` (a connected account's token) for this remote,
    /// instead of any its URL carries. They only apply to http(s) remotes:
    /// ssh authenticates with keys and local repositories need none.
    pub(crate) fn authenticated(mut self, credentials: Option<&Credentials>) -> Self {
        if let Some(c) = credentials
            && self.credential_host.is_some()
        {
            self.credentials = Some(c.clone());
        }
        self
    }

    /// `text` without this remote's credentials, wherever they come from.
    pub(crate) fn redact(&self, text: &str) -> String {
        let mut out = redact_text(text, &self.original);
        // Long enough not to garble unrelated text (as in `redact_text`).
        for secret in self.credentials.iter().map(|c| c.password.as_str()).filter(|s| s.len() >= 6) {
            out = out.replace(secret, "***");
        }
        out
    }
}

impl std::fmt::Debug for RepoUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoUrl")
            .field("git_url", &self.git_url)
            .field("credentials", &self.credentials)
            .field("credential_host", &self.credential_host)
            .field("local_path", &self.local_path)
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

/// Credentials of an http(s) remote, handed to git by a credential helper
/// through its environment (exported as `GitCredentials`).
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(***)")
    }
}

/// Environment variables the credential helper reads.
const USERNAME_VAR: &str = "FERRY_GIT_USERNAME";
const PASSWORD_VAR: &str = "FERRY_GIT_PASSWORD";
const HOST_VAR: &str = "FERRY_GIT_HOST";
/// Answers git's `get` requests from the environment, for the remote's own
/// host only (git describes what it wants credentials for on stdin:
/// `protocol=…`, `host=…`). Only variable names appear on command lines,
/// never values.
const CREDENTIAL_HELPER: &str = "!f() { test \"$1\" = get || exit 0; \
     grep -ixF \"host=$FERRY_GIT_HOST\" >/dev/null || exit 0; \
     printf 'username=%s\\npassword=%s\\n' \"$FERRY_GIT_USERNAME\" \"$FERRY_GIT_PASSWORD\"; }; f";

/// Split `https://user:pass@host/path` into the URL without userinfo and the
/// credentials. Other schemes (ssh users are not secrets) are left alone.
fn split_credentials(url: &str, display: &str) -> Result<(String, Option<Credentials>), GitError> {
    let Some(idx) = url.find("://") else { return Ok((url.to_string(), None)) };
    if !matches!(url[..idx].to_ascii_lowercase().as_str(), "http" | "https") {
        return Ok((url.to_string(), None));
    }
    let rest = &url[idx + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].rfind('@') else { return Ok((url.to_string(), None)) };
    let (user, pass) = match rest[..at].split_once(':') {
        Some((u, p)) => (u, p),
        None => (&rest[..at], ""),
    };
    let decode = |s: &str| percent_decode(s).filter(|d| !d.contains(['\n', '\r', '\0']));
    let (Some(username), Some(password)) = (decode(user), decode(pass)) else {
        return Err(GitError::Invalid(format!("invalid credentials in repository URL '{display}'")));
    };
    let stripped = format!("{}{}", &url[..idx + 3], &rest[at + 1..]);
    if username.is_empty() && password.is_empty() {
        return Ok((stripped, None));
    }
    Ok((stripped, Some(Credentials { username, password })))
}

/// `%XX` decoding (what git applies to URL userinfo); `None` when invalid.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
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
                let (git_url, credentials) = split_credentials(u, &display)?;
                let credential_host = credential_host(&git_url);
                Ok(RepoUrl {
                    git_url,
                    credentials,
                    credential_host,
                    original: u.to_string(),
                    local_path: None,
                    display,
                })
            }
            "file" => {
                let path = PathBuf::from(&u[idx + 3..]);
                Ok(RepoUrl {
                    git_url: u.to_string(),
                    credentials: None,
                    credential_host: None,
                    original: u.to_string(),
                    local_path: Some(path),
                    display,
                })
            }
            _ => Err(GitError::Invalid(format!(
                "unsupported repository URL scheme '{scheme}' (use https, ssh, git, file or a local path)"
            ))),
        };
    }
    if is_scp_like(u) {
        return Ok(RepoUrl {
            git_url: u.to_string(),
            credentials: None,
            credential_host: None,
            original: u.to_string(),
            local_path: None,
            display,
        });
    }
    let path = Path::new(u);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::path::absolute(path).map_err(|e| GitError::Invalid(format!("invalid repository path '{u}': {e}")))?
    };
    Ok(RepoUrl {
        git_url: abs.to_string_lossy().into_owned(),
        credentials: None,
        credential_host: None,
        original: u.to_string(),
        local_path: Some(abs),
        display,
    })
}

/// `host[:port]` of an http(s) URL without userinfo, as git names it to
/// credential helpers (`host=…`). `None` for other schemes.
fn credential_host(git_url: &str) -> Option<String> {
    let idx = git_url.find("://")?;
    if !matches!(git_url[..idx].to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let rest = &git_url[idx + 3..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    (!authority.is_empty()).then(|| authority.to_string())
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

/// [`git_cmd`] for talking to `url`'s remote: its credentials (if any) are
/// served by a helper reading them from the environment, to the remote's
/// own host only. Must be called before adding the subcommand (it adds
/// global `-c` options).
fn git_remote_cmd(url: &RepoUrl) -> Command {
    let mut c = git_cmd();
    if let (Some(creds), Some(host)) = (&url.credentials, &url.credential_host) {
        // The empty value resets helpers configured elsewhere (a keychain
        // could otherwise answer first with other credentials).
        c.args(["-c", "credential.helper=", "-c"]).arg(format!("credential.helper={CREDENTIAL_HELPER}"));
        c.env(USERNAME_VAR, &creds.username).env(PASSWORD_VAR, &creds.password).env(HOST_VAR, host);
    }
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
        Err(e) => Err(GitError::Failed(url.redact(&format!("{what} ({}) failed: {e}", url.display)))),
    }
}

/// The most meaningful line of git's stderr (`fatal:` / `error:`), redacted.
pub(crate) fn git_failure_reason(out: &Output, url: &RepoUrl) -> String {
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
    let mut reason = url.redact(&reason);
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
    let mut cmd = git_remote_cmd(&url);
    cmd.args(["ls-remote", "--refs"]).arg(&url.git_url).arg(&refname);
    let out = run_git(cmd, "git ls-remote", &url, None, LS_REMOTE_TIMEOUT).await?;
    if !out.status.success() {
        return Err(GitError::Failed(format!(
            "cannot list branches of {}: {}",
            url.display,
            git_failure_reason(&out, &url)
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let found = stdout
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .find(|(_, name)| name.trim() == refname)
        .map(|(sha, _)| sha.trim().to_string());
    match found {
        Some(sha) => Ok(sha),
        None => Err(branch_not_found(&url, branch, None).await),
    }
}

/// What a remote says about its branches (`git ls-remote --symref`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteBranches {
    /// The branch `HEAD` points to (the repository's default branch).
    pub default: Option<String>,
    /// Every branch, in the remote's order (by name).
    pub branches: Vec<String>,
}

/// The branches of `repo_url`, asked from the remote itself, with
/// `credentials` when it is an http(s) remote that needs them (see
/// [`RepoUrl::authenticated`]).
pub(crate) async fn list_remote_branches(
    repo_url: &str,
    credentials: Option<&Credentials>,
    timeout: Duration,
) -> Result<RemoteBranches, GitError> {
    let url = prepare_repo_url(repo_url)?.authenticated(credentials);
    check_local_exists(&url).await?;
    let mut cmd = git_remote_cmd(&url);
    cmd.args(["ls-remote", "--symref"]).arg(&url.git_url).args(["HEAD", "refs/heads/*"]);
    let out = run_git(cmd, "git ls-remote", &url, None, timeout).await?;
    if !out.status.success() {
        return Err(GitError::Failed(format!(
            "cannot list the branches of {}: {}",
            url.display,
            git_failure_reason(&out, &url)
        )));
    }
    Ok(parse_remote_branches(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_remote_branches(stdout: &str) -> RemoteBranches {
    let mut out = RemoteBranches::default();
    for (target, name) in stdout.lines().filter_map(|l| l.split_once('\t')) {
        let name = name.trim();
        if name == "HEAD" {
            if let Some(b) = target.strip_prefix("ref:").map(str::trim).and_then(|t| t.strip_prefix("refs/heads/"))
                && validate_branch(b).is_ok()
            {
                out.default = Some(b.to_string());
            }
        } else if let Some(b) = name.strip_prefix("refs/heads/")
            && validate_branch(b).is_ok()
            && !out.branches.iter().any(|x| x == b)
        {
            out.branches.push(b.to_string());
        }
    }
    out
}

/// The "branch not found" error, with a hint about the branches the remote
/// does have (its default branch, e.g. `master` for a service left on the
/// default `main`). The hint is best effort: an unreachable remote just
/// yields the plain message.
async fn branch_not_found(url: &RepoUrl, branch: &str, cancel: Option<&CancellationToken>) -> GitError {
    let mut cmd = git_remote_cmd(url);
    cmd.args(["ls-remote", "--symref"]).arg(&url.git_url).args(["HEAD", "refs/heads/*"]);
    let remote = match run_git(cmd, "git ls-remote", url, cancel, LS_REMOTE_TIMEOUT).await {
        Ok(out) if out.status.success() => Some(parse_remote_branches(&String::from_utf8_lossy(&out.stdout))),
        Err(GitError::Canceled) => return GitError::Canceled,
        _ => None,
    };
    let hint = remote.map(|r| branch_hint(&r, branch)).unwrap_or_default();
    GitError::NotFound(format!("branch '{branch}' not found in {}{hint}", url.display))
}

fn branch_hint(remote: &RemoteBranches, branch: &str) -> String {
    const LISTED: usize = 5;
    if let Some(default) = remote.default.as_deref().filter(|d| *d != branch) {
        return format!(" (the default branch is '{default}')");
    }
    if remote.branches.is_empty() {
        return " (the repository has no branches yet: push a commit first)".to_string();
    }
    let shown: Vec<String> = remote.branches.iter().take(LISTED).map(|b| format!("'{b}'")).collect();
    let more = if remote.branches.len() > LISTED { ", …" } else { "" };
    format!(" (branches: {}{more})", shown.join(", "))
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
            git_failure_reason(&out, url)
        )));
    }
    let mut cmd = git_cmd();
    cmd.arg(git_dir_arg(cache)).args(["config", "remote.origin.url"]).arg(&url.git_url);
    let out = run_git(cmd, "git config", url, Some(cancel), LOCAL_TIMEOUT).await?;
    if !out.status.success() {
        let _ = remove_dir_async(cache.to_path_buf()).await;
        return Err(GitError::Failed(format!("cannot configure git cache: {}", git_failure_reason(&out, url))));
    }
    Ok(())
}

/// `git fetch origin <refspecs>` into the cache.
async fn fetch(cache: &Path, url: &RepoUrl, refspecs: &[String], cancel: &CancellationToken) -> Result<(), GitError> {
    let mut cmd = git_remote_cmd(url);
    cmd.arg(git_dir_arg(cache)).args(["fetch", "--quiet", "--no-tags", "--force", "origin"]).args(refspecs);
    let out = run_git(cmd, "git fetch", url, Some(cancel), FETCH_TIMEOUT).await?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = git_failure_reason(&out, url);
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
        return Err(GitError::Failed(format!("cannot read commit {sha}: {}", git_failure_reason(&out, url))));
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
/// the source's branch / commit into `dest`.
pub(crate) async fn checkout(
    cache: &Path,
    source: Source<'_>,
    dest: &Path,
    logs: &LogSink,
    cancel: &CancellationToken,
) -> Result<Checkout, GitError> {
    let Source { repo_url, credentials, branch, commit } = source;
    let url = prepare_repo_url(repo_url)?.authenticated(credentials);
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
                Err(GitError::NotFound(_)) => return Err(branch_not_found(&url, branch, Some(cancel)).await),
                Err(e) => return Err(e),
            }
            match rev_parse(cache, &url, &format!("refs/remotes/origin/{branch}"), cancel).await? {
                Some(sha) => sha,
                None => return Err(branch_not_found(&url, branch, Some(cancel)).await),
            }
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
        match tokio::task::spawn_blocking(move || extract_tar(bridge, &dest, LinkPolicy::Skip, Limits::GIT, &token))
            .await
        {
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
                Err(GitError::Failed(format!("git archive failed: {}", git_failure_reason(&out, url))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`super::checkout`] of a URL without separate credentials.
    async fn checkout(
        cache: &Path,
        repo_url: &str,
        branch: &str,
        commit: Option<&str>,
        dest: &Path,
        logs: &LogSink,
        cancel: &CancellationToken,
    ) -> Result<Checkout, GitError> {
        super::checkout(cache, Source { repo_url, credentials: None, branch, commit }, dest, logs, cancel).await
    }

    #[test]
    fn classifies_urls() {
        let https = prepare_repo_url("https://u:tok@github.com/a/b.git").unwrap();
        // Credentials never reach git's command line.
        assert_eq!(https.git_url, "https://github.com/a/b.git");
        assert_eq!(https.credentials, Some(Credentials { username: "u".into(), password: "tok".into() }));
        assert_eq!(https.credential_host.as_deref(), Some("github.com"));
        assert_eq!(https.display, "https://***@github.com/a/b.git");
        assert!(!format!("{https:?}").contains("tok@") && !format!("{:?}", https.credentials).contains("tok"));
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
    fn credentials_are_split_off_http_urls() {
        let split = |u: &str| prepare_repo_url(u).map(|r| (r.git_url, r.credentials));
        let creds = |u: &str, p: &str| Some(Credentials { username: u.into(), password: p.into() });
        // Percent-encoded userinfo is decoded, like git does.
        assert_eq!(
            split("https://deploy:p%40ss%20word@git.example.com:8443/org/repo.git?x=1").unwrap(),
            ("https://git.example.com:8443/org/repo.git?x=1".to_string(), creds("deploy", "p@ss word"))
        );
        // A lone token as username.
        assert_eq!(
            split("http://ghp_token@github.com/a/b").unwrap(),
            ("http://github.com/a/b".to_string(), creds("ghp_token", ""))
        );
        // No userinfo, or an `@` in the path: untouched.
        assert_eq!(split("https://github.com/a/b@c").unwrap(), ("https://github.com/a/b@c".to_string(), None));
        // ssh users are not secrets: untouched.
        assert_eq!(split("ssh://git@github.com/a/b").unwrap(), ("ssh://git@github.com/a/b".to_string(), None));
        // Invalid escapes or control characters are refused, without leaking.
        for bad in ["https://u:bad%zzsecret@h/r", "https://u:line%0Abreak-secret@h/r"] {
            let err = split(bad).unwrap_err().message();
            assert!(err.contains("invalid credentials") && !err.contains("secret"), "{err}");
        }
    }

    #[test]
    fn remote_commands_carry_credentials_in_the_environment_only() {
        let url = prepare_repo_url("https://deploy:s3cr3t-pass@git.example.com/org/repo.git").unwrap();
        let mut cmd = git_remote_cmd(&url);
        cmd.args(["ls-remote", "--refs"]).arg(&url.git_url);
        let std = cmd.as_std();
        let argv: Vec<String> = std.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!argv.iter().any(|a| a.contains("s3cr3t") || a.contains("deploy")), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-c", "credential.helper="]), "{argv:?}");
        assert!(argv.iter().any(|a| a.starts_with("credential.helper=!") && a.contains(PASSWORD_VAR)), "{argv:?}");
        let env = |k: &str| std.get_envs().find(|(n, _)| *n == k).and_then(|(_, v)| v).map(|v| v.to_owned());
        assert_eq!(env(USERNAME_VAR).as_deref(), Some(std::ffi::OsStr::new("deploy")));
        assert_eq!(env(PASSWORD_VAR).as_deref(), Some(std::ffi::OsStr::new("s3cr3t-pass")));
        // ...for the remote's host only.
        assert_eq!(env(HOST_VAR).as_deref(), Some(std::ffi::OsStr::new("git.example.com")));
        assert!(CREDENTIAL_HELPER.contains(USERNAME_VAR) && CREDENTIAL_HELPER.contains(HOST_VAR));
        // Without credentials nothing is added.
        let plain = prepare_repo_url("https://github.com/a/b").unwrap();
        assert!(
            !git_remote_cmd(&plain).as_std().get_args().any(|a| a.to_string_lossy().contains("credential.helper=!"))
        );
    }

    #[test]
    fn separate_credentials_apply_to_http_remotes_only() {
        let token = Credentials { username: "x-access-token".into(), password: "ghp_s3cr3tT0ken".into() };
        let https = prepare_repo_url("https://github.com:8443/a/b.git").unwrap().authenticated(Some(&token));
        assert_eq!(https.credentials.as_ref(), Some(&token));
        assert_eq!(https.credential_host.as_deref(), Some("github.com:8443"));
        // The URL stays as it was: nothing to redact in it, but the token is
        // masked wherever it shows up.
        assert_eq!(
            (https.git_url.as_str(), https.display.as_str()),
            (https.original.as_str(), https.original.as_str())
        );
        assert_eq!(https.redact("remote said: ghp_s3cr3tT0ken is wrong"), "remote said: *** is wrong");
        // They replace what the URL carries.
        let both = prepare_repo_url("https://old:old-secret@github.com/a/b").unwrap().authenticated(Some(&token));
        assert_eq!(both.credentials.as_ref(), Some(&token));
        assert_eq!(both.redact("old-secret ghp_s3cr3tT0ken"), "*** ***");
        // ssh, git:// and local repositories never get them.
        for other in ["git@github.com:a/b.git", "ssh://git@github.com/a/b", "git://github.com/a/b", "/srv/repo"] {
            let url = prepare_repo_url(other).unwrap().authenticated(Some(&token));
            assert_eq!((url.credentials, url.credential_host), (None, None), "{other}");
        }
        assert_eq!(prepare_repo_url("https://github.com/a/b").unwrap().authenticated(None).credentials, None);
    }

    fn base64(input: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A dumb-HTTP git server for `repo` that demands Basic credentials.
    /// Returns its port and whether an authorized request was seen.
    async fn auth_git_server(repo: PathBuf, expected: String) -> (u16, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let authorized = std::sync::Arc::new(AtomicBool::new(false));
        let seen = authorized.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (repo, expected, seen) = (repo.clone(), expected.clone(), seen.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let req = String::from_utf8_lossy(&buf).into_owned();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/");
                    let auth_ok = req.lines().any(|l| {
                        l.split_once(':')
                            .is_some_and(|(k, v)| k.eq_ignore_ascii_case("authorization") && v.trim() == expected)
                    });
                    let file = path.strip_prefix("/repo.git/").map(|rel| repo.join(rel));
                    let response = match (auth_ok, file.and_then(|f| std::fs::read(f).ok())) {
                        (false, _) => b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"t\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                        (true, Some(body)) => {
                            seen.store(true, Ordering::SeqCst);
                            let mut r = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            r.extend_from_slice(&body);
                            r
                        }
                        (true, None) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = sock.write_all(&response).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (port, authorized)
    }

    #[tokio::test]
    async fn http_credentials_reach_the_server_but_not_argv_or_disk() {
        let (src, _, head) = sample_repo();
        // A bare copy served over dumb HTTP.
        let served = tempfile::tempdir().unwrap();
        let bare = served.path().join("repo.git");
        git_in(served.path(), &["clone", "-q", "--bare", &src.path().to_string_lossy(), "repo.git"]);
        git_in(&bare, &["update-server-info"]);
        let expected = format!("Basic {}", base64(b"deploy:s3cr3t pass"));
        let (port, authorized) = auth_git_server(bare, expected).await;
        let url = format!("http://deploy:s3cr3t%20pass@127.0.0.1:{port}/repo.git");

        assert_eq!(ls_remote_branch(&url, "main").await.unwrap(), head);
        let work = tempfile::tempdir().unwrap();
        let cache = work.path().join("cache");
        let dest = work.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let (logs, mut rx) = LogSink::channel();
        let co = checkout(&cache, &url, "main", None, &dest, &logs, &CancellationToken::new())
            .await
            .unwrap_or_else(|e| panic!("checkout failed: {}", e.message()));
        assert_eq!(co.sha, head);
        assert!(authorized.load(std::sync::atomic::Ordering::SeqCst));
        // The cache's config holds the URL without credentials.
        let config = std::fs::read_to_string(cache.join("config")).unwrap();
        assert!(config.contains(&format!("http://127.0.0.1:{port}/repo.git")), "{config}");
        assert!(!config.contains("s3cr3t") && !config.contains("deploy"), "{config}");
        for l in lines(&mut rx) {
            assert!(!l.contains("s3cr3t"), "{l}");
        }

        // Wrong credentials: a clean, redacted failure (no prompt, no hang).
        let bad = format!("http://deploy:wrong-s3cr3t@127.0.0.1:{port}/repo.git");
        let err = ls_remote_branch(&bad, "main").await.unwrap_err().message();
        assert!(!err.contains("wrong-s3cr3t"), "{err}");
    }

    #[tokio::test]
    async fn separate_credentials_reach_the_server_but_not_argv_logs_or_disk() {
        let (src, _, head) = sample_repo();
        let served = tempfile::tempdir().unwrap();
        let bare = served.path().join("repo.git");
        git_in(served.path(), &["clone", "-q", "--bare", &src.path().to_string_lossy(), "repo.git"]);
        git_in(&bare, &["update-server-info"]);
        let expected = format!("Basic {}", base64(b"x-access-token:ghp_s3cr3tT0ken"));
        let (port, authorized) = auth_git_server(bare, expected).await;
        // A plain URL: the token of a connected account comes next to it.
        let url = format!("http://127.0.0.1:{port}/repo.git");
        let token = Credentials { username: "x-access-token".into(), password: "ghp_s3cr3tT0ken".into() };
        let source = |credentials| Source { repo_url: &url, credentials, branch: "main", commit: None };
        let cancel = CancellationToken::new();

        let work = tempfile::tempdir().unwrap();
        let (cache, dest) = (work.path().join("cache"), work.path().join("out"));
        std::fs::create_dir_all(&dest).unwrap();
        let (logs, mut rx) = LogSink::channel();
        let co = super::checkout(&cache, source(Some(&token)), &dest, &logs, &cancel)
            .await
            .unwrap_or_else(|e| panic!("checkout failed: {}", e.message()));
        assert_eq!(co.sha, head);
        assert!(authorized.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(std::fs::read_to_string(dest.join("app.txt")).unwrap(), "v2");
        let config = std::fs::read_to_string(cache.join("config")).unwrap();
        assert!(!config.contains("ghp_") && !config.contains("x-access-token"), "{config}");
        let logged = lines(&mut rx);
        assert!(logged.contains(&format!("==> Cloning from {url} (branch main)")), "{logged:?}");
        assert!(!logged.iter().any(|l| l.contains("ghp_")), "{logged:?}");

        // Without the token, or with a wrong one: a clean failure that never
        // echoes the token (no prompt, no hang).
        let other = work.path().join("other");
        let err = super::checkout(&other, source(None), &dest, &logs, &cancel).await.unwrap_err().message();
        assert!(err.contains(&url), "{err}");
        let wrong = Credentials { username: "x-access-token".into(), password: "ghp_wr0ngT0ken".into() };
        let err = super::checkout(&other, source(Some(&wrong)), &dest, &logs, &cancel).await.unwrap_err().message();
        assert!(err.contains(&url) && !err.contains("ghp_"), "{err}");
    }

    /// What a connected account is cloned with: `value` as the password.
    fn account(value: &str) -> Credentials {
        Credentials { username: "x-access-token".into(), password: value.into() }
    }

    /// What the private remote of `lists_the_branches_of_a_remote` lets in,
    /// and what it doesn't.
    const ADMITTED: &str = "admitted-by-the-remote";
    const REFUSED: &str = "refused-by-the-remote";

    #[tokio::test]
    async fn lists_the_branches_of_a_remote() {
        let (repo, _, _) = sample_repo();
        git_in(repo.path(), &["branch", "dev"]);
        git_in(repo.path(), &["branch", "feature/login"]);
        let local = repo.path().to_string_lossy().into_owned();
        let remote = list_remote_branches(&local, None, LS_REMOTE_TIMEOUT).await.unwrap();
        assert_eq!(remote.default.as_deref(), Some("main"));
        assert_eq!(remote.branches, vec!["dev", "feature/login", "main"]);

        // A private http remote lists them to its credentials only, and
        // never echoes them.
        let served = tempfile::tempdir().unwrap();
        git_in(served.path(), &["clone", "-q", "--bare", &local, "repo.git"]);
        git_in(&served.path().join("repo.git"), &["update-server-info"]);
        let expected = format!("Basic {}", base64(format!("x-access-token:{ADMITTED}").as_bytes()));
        let (port, authorized) = auth_git_server(served.path().join("repo.git"), expected).await;
        let url = format!("http://127.0.0.1:{port}/repo.git");
        let err = list_remote_branches(&url, None, LS_REMOTE_TIMEOUT).await.unwrap_err().message();
        assert!(err.starts_with(&format!("cannot list the branches of {url}: ")), "{err}");
        let wrong = account(REFUSED);
        let err = list_remote_branches(&url, Some(&wrong), LS_REMOTE_TIMEOUT).await.unwrap_err().message();
        assert!(err.contains(&url) && !err.contains(REFUSED), "{err}");
        assert!(!authorized.load(std::sync::atomic::Ordering::SeqCst));
        let right = account(ADMITTED);
        let remote = list_remote_branches(&url, Some(&right), LS_REMOTE_TIMEOUT).await.unwrap();
        assert!(authorized.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(remote.branches, vec!["dev", "feature/login", "main"]);

        // What isn't a repository says so.
        let missing = list_remote_branches("/nonexistent/ferry-repo", None, LS_REMOTE_TIMEOUT).await.unwrap_err();
        assert!(matches!(&missing, GitError::NotFound(m) if m.contains("does not exist")), "{missing:?}");
        let invalid = list_remote_branches("ext::sh -c id", None, LS_REMOTE_TIMEOUT).await.unwrap_err();
        assert!(matches!(invalid, GitError::Invalid(_)), "{invalid:?}");
    }

    /// A server answering every request with a redirect to the same path on
    /// `http://127.0.0.1:<to_port>`.
    async fn redirecting_server(to_port: u16) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let req = String::from_utf8_lossy(&buf).into_owned();
                    let target = req.split_whitespace().nth(1).unwrap_or("/");
                    let response = format!(
                        "HTTP/1.1 301 Moved Permanently\r\nLocation: http://127.0.0.1:{to_port}{target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn credentials_are_not_sent_to_the_host_a_remote_redirects_to() {
        let (src, _, head) = sample_repo();
        let served = tempfile::tempdir().unwrap();
        let bare = served.path().join("repo.git");
        git_in(served.path(), &["clone", "-q", "--bare", &src.path().to_string_lossy(), "repo.git"]);
        git_in(&bare, &["update-server-info"]);
        // The repository lives on one host (`authorized`: it was sent the
        // credentials); the configured remote redirects there.
        let expected = format!("Basic {}", base64(b"x-access-token:ghp_s3cr3tT0ken"));
        let (target_port, authorized) = auth_git_server(bare, expected).await;
        let port = redirecting_server(target_port).await;
        let token = Credentials { username: "x-access-token".into(), password: "ghp_s3cr3tT0ken".into() };
        let cancel = CancellationToken::new();
        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();

        for (i, url) in [
            // A connected account's token next to the URL...
            (0, format!("http://127.0.0.1:{port}/repo.git")),
            // ...and credentials in the URL itself.
            (1, format!("http://x-access-token:ghp_s3cr3tT0ken@127.0.0.1:{port}/repo.git")),
        ] {
            let credentials = (i == 0).then_some(&token);
            let source = Source { repo_url: &url, credentials, branch: "main", commit: None };
            let cache = work.path().join(format!("cache{i}"));
            let err = super::checkout(&cache, source, &dest, &LogSink::noop(), &cancel).await.unwrap_err().message();
            assert!(!err.contains("ghp_"), "{err}");
            assert!(!authorized.load(std::sync::atomic::Ordering::SeqCst), "the redirect target got the credentials");
        }

        // Asked directly, the same host is given them.
        let url = format!("http://127.0.0.1:{target_port}/repo.git");
        let source = Source { repo_url: &url, credentials: Some(&token), branch: "main", commit: None };
        let cache = work.path().join("direct");
        let co = super::checkout(&cache, source, &dest, &LogSink::noop(), &cancel).await.unwrap();
        assert_eq!(co.sha, head);
        assert!(authorized.load(std::sync::atomic::Ordering::SeqCst));
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
        let r = git_failure_reason(&out, &prepare_repo_url(url).unwrap());
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
        let expected = format!("branch 'nope' not found in {url} (the default branch is 'main')");
        assert!(matches!(&err, GitError::NotFound(m) if m == &expected), "{err:?}");

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

    #[tokio::test]
    async fn missing_branch_names_the_default_branch() {
        // Regression: a master-only repository with the service on the
        // default `main` said only "branch 'main' not found".
        let d = tempfile::tempdir().unwrap();
        git_in(d.path(), &["init", "-q", "-b", "master"]);
        std::fs::write(d.path().join("a.txt"), "a").unwrap();
        git_in(d.path(), &["add", "-A"]);
        git_in(d.path(), &["commit", "-q", "-m", "init"]);
        git_in(d.path(), &["branch", "dev"]);
        let url = d.path().to_string_lossy().into_owned();
        let expected = format!("branch 'main' not found in {url} (the default branch is 'master')");

        let err = ls_remote_branch(&url, "main").await.unwrap_err();
        assert!(matches!(&err, GitError::NotFound(m) if m == &expected), "{err:?}");

        let work = tempfile::tempdir().unwrap();
        let dest = work.path().join("b");
        std::fs::create_dir_all(&dest).unwrap();
        let cancel = CancellationToken::new();
        let err = checkout(&work.path().join("cache"), &url, "main", None, &dest, &LogSink::noop(), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(&err, GitError::NotFound(m) if m == &expected), "{err:?}");
        // Through the file:// form (bare remote) too.
        let bare = work.path().join("bare.git");
        git_in(work.path(), &["clone", "-q", "--bare", &url, &bare.to_string_lossy()]);
        let file_url = format!("file://{}", bare.display());
        let err = ls_remote_branch(&file_url, "main").await.unwrap_err();
        assert!(err.message().ends_with("(the default branch is 'master')"), "{err:?}");

        // An empty repository says so.
        let empty = tempfile::tempdir().unwrap();
        git_in(empty.path(), &["init", "-q", "-b", "main"]);
        let err = ls_remote_branch(&empty.path().to_string_lossy(), "main").await.unwrap_err();
        assert!(err.message().ends_with("(the repository has no branches yet: push a commit first)"), "{err:?}");
    }

    #[test]
    fn remote_branch_hints() {
        let out = "ref: refs/heads/master\tHEAD\n\
                   1111111111111111111111111111111111111111\tHEAD\n\
                   1111111111111111111111111111111111111111\trefs/heads/dev\n\
                   1111111111111111111111111111111111111111\trefs/heads/master\n\
                   1111111111111111111111111111111111111111\trefs/remotes/x/refs/heads/zz\n";
        let r = parse_remote_branches(out);
        assert_eq!(r.default.as_deref(), Some("master"));
        assert_eq!(r.branches, vec!["dev".to_string(), "master".to_string()]);
        assert_eq!(branch_hint(&r, "main"), " (the default branch is 'master')");
        // HEAD on the missing branch itself (unborn): list what exists.
        assert_eq!(branch_hint(&r, "master"), " (branches: 'dev', 'master')");
        let many = RemoteBranches { default: None, branches: (1..=7).map(|i| format!("b{i}")).collect() };
        assert_eq!(branch_hint(&many, "main"), " (branches: 'b1', 'b2', 'b3', 'b4', 'b5', …)");
        assert_eq!(
            branch_hint(&RemoteBranches::default(), "main"),
            " (the repository has no branches yet: push a commit first)"
        );
        // Names that could be mistaken for anything else are ignored.
        let r = parse_remote_branches("ref: refs/heads/-evil\tHEAD\nx\trefs/heads/a b\n");
        assert_eq!(r, RemoteBranches::default());
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
