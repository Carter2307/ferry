//! Command implementations. Every command talks to the HTTP API through
//! [`Client`] and prints either formatted text or (with `--json`) the raw JSON.

mod auth;
mod blueprint;
mod db;
mod deploys;
mod env;
mod jobs;
mod logs;
mod services;
mod up;

use std::fmt;
use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ferry_core::dto::ServiceView;
use ferry_core::{Deploy, DeployStatus, JobRun, JobStatus, LogLine};
use serde_json::Value;

use crate::cli::{Cli, Command};
use crate::client::{ApiError, Client, IdleTimeout};
use crate::config::Settings;
use crate::output::{self, Color, errln, outln};

/// Everything a command needs.
pub struct Ctx {
    pub client: Client,
    /// `--json`: print raw API JSON.
    pub json: bool,
}

/// Ends the process with this exit code without printing an error (the
/// command already explained what happened, e.g. "Deploy … failed").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit(pub u8);

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "exit status {}", self.0)
    }
}

impl std::error::Error for Exit {}

/// Run a parsed command line.
pub async fn run(cli: Cli, settings: Settings) -> Result<()> {
    let json = cli.global.json;
    match cli.command {
        Command::Login => auth::login(&settings, json).await,
        command => {
            let token = settings.require_token()?;
            let ctx = Ctx { client: Client::new(&settings.server, token)?, json };
            dispatch(&ctx, command).await
        }
    }
}

async fn dispatch(ctx: &Ctx, command: Command) -> Result<()> {
    use crate::cli::{BlueprintCommand, DbCommand, DomainsCommand, EnvCommand, EnvGroupCommand};
    match command {
        Command::Login => bail!("internal error: 'login' needs no API client"),
        Command::Info => auth::info(ctx).await,
        Command::Services => services::list(ctx).await,
        Command::Create(a) => services::create(ctx, a).await,
        Command::Show(a) => services::show(ctx, &a.name).await,
        Command::Update(a) => services::update(ctx, a).await,
        Command::Delete(a) => services::delete(ctx, &a.name, a.yes).await,
        Command::Deploy(a) => deploys::deploy(ctx, a).await,
        Command::Up(a) => up::up(ctx, a).await,
        Command::Deploys(a) => deploys::list(ctx, &a.name, a.limit).await,
        Command::Cancel(a) => deploys::cancel(ctx, &a.deploy_id).await,
        Command::Rollback(a) => deploys::rollback(ctx, &a.name, &a.deploy_id, a.follow).await,
        Command::Restart(a) => deploys::restart(ctx, &a.name, a.follow).await,
        Command::Suspend(a) => services::suspend(ctx, &a.name, true).await,
        Command::Resume(a) => services::suspend(ctx, &a.name, false).await,
        Command::Scale(a) => services::scale(ctx, &a.name, a.instances).await,
        Command::Status(a) => services::status(ctx, &a.name).await,
        Command::Logs(a) => logs::logs(ctx, a).await,
        Command::Env(a) => match a.command {
            Some(EnvCommand::Ls(n)) => env::list(ctx, &n.name, n.effective).await,
            Some(EnvCommand::Set(s)) => env::set(ctx, &s.name, s.vars, !s.no_restart, s.follow).await,
            Some(EnvCommand::Unset(u)) => env::unset(ctx, &u.name, u.keys, !u.no_restart, u.follow).await,
            None => env::list(ctx, a.name.as_deref().unwrap_or_default(), a.effective).await,
        },
        Command::Domains(a) => match a.command {
            Some(DomainsCommand::Ls(n)) => services::domains_list(ctx, &n.name).await,
            Some(DomainsCommand::Add(d)) => services::domains_add(ctx, &d.name, &d.domain).await,
            Some(DomainsCommand::Rm(d)) => services::domains_rm(ctx, &d.name, &d.domain).await,
            None => services::domains_list(ctx, a.name.as_deref().unwrap_or_default()).await,
        },
        Command::Run(a) => jobs::run(ctx, &a.name, &a.command, a.follow).await,
        Command::Jobs(a) => jobs::list(ctx, &a.name, a.limit).await,
        Command::Db(c) => match c {
            DbCommand::Create(a) => db::create(ctx, a).await,
            DbCommand::Ls => db::list(ctx).await,
            DbCommand::Show(a) => db::show(ctx, &a.name).await,
            DbCommand::Rm(a) => db::remove(ctx, &a.name, a.yes).await,
        },
        Command::EnvGroup(c) => match c {
            EnvGroupCommand::Create(a) => env::group_create(ctx, &a.name, a.vars).await,
            EnvGroupCommand::Ls => env::group_list(ctx).await,
            EnvGroupCommand::Show(a) => env::group_show(ctx, &a.name).await,
            EnvGroupCommand::Set(a) => env::group_set(ctx, &a.name, a.vars, !a.no_restart).await,
            EnvGroupCommand::Unset(a) => env::group_unset(ctx, &a.name, a.keys, !a.no_restart).await,
            EnvGroupCommand::Rm(a) => env::group_remove(ctx, &a.name, a.yes).await,
            EnvGroupCommand::Link(a) => env::group_link(ctx, &a.service, &a.group).await,
            EnvGroupCommand::Unlink(a) => env::group_unlink(ctx, &a.service, &a.group).await,
        },
        Command::Blueprint(BlueprintCommand::Apply(a)) => blueprint::apply(ctx, a).await,
        Command::Open(a) => services::open(ctx, &a.name).await,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers

/// Pretty-print raw JSON to stdout.
pub fn print_json(value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("formatting JSON")?;
    outln!("{text}")?;
    Ok(())
}

/// Ask for confirmation of a destructive action. `--yes` skips the prompt;
/// without a TTY on stdin the action is refused.
pub async fn confirm(action: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("refusing to {action} without confirmation: re-run with --yes");
    }
    output::write_stderr(format_args!("{} [y/N] ", output::err(Color::Bold, &format!("{}?", capitalize(action)))));
    let answer = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map(|_| line)
    })
    .await
    .context("reading confirmation")?
    .context("reading confirmation")?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        errln!("Aborted.");
        Err(Exit(1).into())
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Print one streamed log line (formatted, or its JSON with `--json`).
fn print_log_event(ctx: &Ctx, data: &str) -> Result<()> {
    if ctx.json {
        outln!("{data}")?;
        return Ok(());
    }
    match serde_json::from_str::<LogLine>(data) {
        Ok(line) => outln!("{}", output::format_log_line(&line, output::stdout_color()))?,
        Err(_) => outln!("{data}")?,
    }
    Ok(())
}

/// What a finite log stream belongs to.
#[derive(Debug, Clone, Copy)]
enum Finite<'a> {
    Deploy(&'a str),
    Job(&'a str),
}

impl Finite<'_> {
    fn segments(&self) -> [&str; 3] {
        match self {
            Finite::Deploy(id) => ["deploys", id, "logs"],
            Finite::Job(id) => ["jobs", id, "logs"],
        }
    }

    /// Has the deploy/job reached a terminal state?
    async fn finished(&self, ctx: &Ctx) -> Result<bool> {
        Ok(match self {
            Finite::Deploy(id) => ctx.client.get::<Deploy>(&["deploys", id], &[]).await?.data.status.is_terminal(),
            Finite::Job(id) => ctx.client.get::<JobRun>(&["jobs", id], &[]).await?.data.status.is_terminal(),
        })
    }
}

/// No bytes at all (the server sends keep-alives every 15s) for this long
/// means the connection is dead.
const STREAM_IDLE: Duration = Duration::from_secs(60);
const MAX_RECONNECTS: u32 = 5;

/// Why a log stream stopped before its `end` event.
enum Interruption {
    /// Connected, but nothing (not even a keep-alive) arrived for a while.
    Idle,
    /// The connection could not be made, broke, or was closed early.
    Lost(anyhow::Error),
}

/// Stream a deploy/job log with `follow=true` until the server's `end` event.
/// If the connection drops, reconnect and skip the lines already printed
/// (the server replays the stored log from the start).
async fn stream_finite(ctx: &Ctx, target: Finite<'_>) -> Result<()> {
    let segments = target.segments();
    let query = [("follow", "true".to_string())];
    let mut printed: u64 = 0;
    let mut failures: u32 = 0;
    loop {
        let interruption = match ctx.client.events(&segments, &query).await {
            Ok(mut stream) => {
                let mut replayed: u64 = 0;
                loop {
                    match stream.next(Some(STREAM_IDLE)).await {
                        Ok(Some(ev)) => match ev.event.as_str() {
                            "log" => {
                                if replayed < printed {
                                    replayed += 1;
                                    continue;
                                }
                                print_log_event(ctx, &ev.data)?;
                                printed += 1;
                                replayed = printed;
                                failures = 0;
                            }
                            "end" => return Ok(()),
                            "error" => errln!("{} {}", output::err(Color::Red, "server:"), ev.data),
                            _ => {}
                        },
                        Ok(None) => break Interruption::Lost(anyhow::anyhow!("the server closed the log stream")),
                        Err(e) if e.downcast_ref::<IdleTimeout>().is_some() => break Interruption::Idle,
                        Err(e) => break Interruption::Lost(e),
                    }
                }
            }
            // A definite client error (404, 401...) won't get better by retrying.
            Err(e) if e.downcast_ref::<ApiError>().is_some_and(|a| a.status < 500) => return Err(e),
            Err(e) => Interruption::Lost(e),
        };
        let finished = target.finished(ctx).await;
        let problem = match interruption {
            // A quiet build step: the server is reachable and the deploy/job
            // is still running, so just reconnect.
            Interruption::Idle if matches!(finished, Ok(false)) => continue,
            Interruption::Idle => anyhow::Error::new(IdleTimeout(STREAM_IDLE)),
            Interruption::Lost(e) => e,
        };
        // Reconnecting replays the stored log (already printed lines are
        // skipped) and, once the deploy/job is finished, ends with `end`. If it
        // finished and a reconnect still brought nothing new, we are done.
        failures += 1;
        let finished = matches!(finished, Ok(true));
        if finished && failures > 1 {
            return Ok(());
        }
        if failures > MAX_RECONNECTS {
            return Err(problem.context("log stream lost"));
        }
        let backoff = if finished { 200 } else { 500 * 2u64.pow(failures.min(4)) };
        tokio::time::sleep(Duration::from_millis(backoff)).await;
    }
}

/// How long to wait for a deploy/job to report a terminal state after its
/// log stream ended.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Follow a deploy's log to the end, then print its final status.
/// Returns `Exit(1)` unless the deploy went live (a deploy that failed, was
/// canceled or whose outcome could not be confirmed is not a success).
pub async fn follow_deploy(ctx: &Ctx, deploy_id: &str) -> Result<()> {
    output::set_interrupt_hint(Some(format!(
        "Stopped following; the deploy continues on the server. Resume with: ferry logs --deploy {deploy_id} -f"
    )));
    let res = stream_finite(ctx, Finite::Deploy(deploy_id)).await;
    output::set_interrupt_hint(None);
    res?;

    let deadline = tokio::time::Instant::now() + SETTLE_TIMEOUT;
    let deploy = loop {
        let d = ctx.client.get::<Deploy>(&["deploys", deploy_id], &[]).await?;
        if d.data.status.is_terminal() || tokio::time::Instant::now() >= deadline {
            break d;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    if ctx.json {
        print_json(&deploy.raw)?;
    } else {
        report_deploy(ctx, &deploy.data).await?;
    }
    match deploy.data.status {
        DeployStatus::Live | DeployStatus::Deactivated => Ok(()),
        _ => Err(Exit(1).into()),
    }
}

async fn report_deploy(ctx: &Ctx, d: &Deploy) -> Result<()> {
    let id = &d.id;
    let reason = d.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default();
    match d.status {
        DeployStatus::Live => {
            let url =
                ctx.client.get::<ServiceView>(&["services", &d.service_id], &[]).await.ok().and_then(|s| s.data.url);
            match url {
                Some(u) => outln!("{} {u}", output::out(Color::Green, &format!("Deploy {id} is live at")))?,
                None => outln!("{}", output::out(Color::Green, &format!("Deploy {id} is live")))?,
            }
        }
        DeployStatus::Deactivated => {
            outln!("Deploy {id} succeeded (already replaced by a newer deploy)")?;
        }
        DeployStatus::BuildFailed | DeployStatus::DeployFailed => {
            outln!("{}", output::out(Color::Red, &format!("Deploy {id} failed ({}){reason}", d.status)))?;
        }
        DeployStatus::Canceled => {
            outln!("{}", output::out(Color::Yellow, &format!("Deploy {id} was canceled{reason}")))?;
        }
        s => outln!("Deploy {id} is still {s}; follow it with: ferry logs --deploy {id} -f")?,
    }
    Ok(())
}

/// Print a freshly queued deploy: follow it, or show how to.
pub async fn queued_deploy(ctx: &Ctx, deploy: &crate::client::Json<Deploy>, follow: bool) -> Result<()> {
    let d = &deploy.data;
    if follow {
        if !ctx.json {
            errln!("Deploy {} {} ({}), streaming logs…", d.id, d.status, d.trigger);
        }
        return follow_deploy(ctx, &d.id).await;
    }
    if ctx.json {
        return print_json(&deploy.raw);
    }
    outln!(
        "Deploy {} {} ({})",
        d.id,
        output::out_opt(output::deploy_status_color(d.status), d.status.as_str()),
        d.trigger
    )?;
    outln!("Follow it with: ferry logs --deploy {} -f", d.id)?;
    Ok(())
}

/// Follow a job's output to the end, then print its result.
/// Returns `Exit(1)` unless the job succeeded.
pub async fn follow_job(ctx: &Ctx, job_id: &str) -> Result<()> {
    output::set_interrupt_hint(Some(format!(
        "Stopped following; the job continues on the server. Resume with: ferry logs --job {job_id} -f"
    )));
    let res = stream_finite(ctx, Finite::Job(job_id)).await;
    output::set_interrupt_hint(None);
    res?;

    let deadline = tokio::time::Instant::now() + SETTLE_TIMEOUT;
    let job = loop {
        let j = ctx.client.get::<JobRun>(&["jobs", job_id], &[]).await?;
        if j.data.status.is_terminal() || tokio::time::Instant::now() >= deadline {
            break j;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    if ctx.json {
        print_json(&job.raw)?;
    } else {
        let j = &job.data;
        let code = j.exit_code.map(|c| format!(" (exit code {c})")).unwrap_or_default();
        let reason = j.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default();
        match j.status {
            JobStatus::Succeeded => outln!("{}", output::out(Color::Green, &format!("Job {} succeeded{code}", j.id)))?,
            JobStatus::Failed => outln!("{}", output::out(Color::Red, &format!("Job {} failed{code}{reason}", j.id)))?,
            JobStatus::Canceled => {
                outln!("{}", output::out(Color::Yellow, &format!("Job {} was canceled{reason}", j.id)))?
            }
            s => outln!("Job {} is still {s}; follow it with: ferry logs --job {} -f", j.id, j.id)?,
        }
    }
    match job.data.status {
        JobStatus::Succeeded => Ok(()),
        _ => Err(Exit(1).into()),
    }
}

/// `-` for empty values in detail views.
pub fn or_dash(v: Option<&str>) -> String {
    match v {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalize_first_letter() {
        assert_eq!(capitalize("delete service 'web'"), "Delete service 'web'");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn dash_for_missing() {
        assert_eq!(or_dash(None), "-");
        assert_eq!(or_dash(Some("")), "-");
        assert_eq!(or_dash(Some("x")), "x");
    }
}
