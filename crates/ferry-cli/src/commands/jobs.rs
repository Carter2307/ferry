//! `ferry run` (one-off jobs / trigger a cron job) and `ferry jobs`.

use anyhow::Result;
use ferry_core::JobRun;
use ferry_core::dto::RunJobRequest;

use super::{Ctx, follow_job, print_json};
use crate::output::{self, Cell, Table, errln, outln};

/// Quote one word for `sh` when needed (single quotes, `'` → `'\''`).
pub fn shell_quote(word: &str) -> String {
    let safe = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c));
    if safe { word.to_string() } else { format!("'{}'", word.replace('\'', r"'\''")) }
}

/// The job command for `ferry run NAME -- ARGS...`: a single argument is
/// taken verbatim as a shell command line (`-- "npm run a && npm run b"`);
/// several arguments are quoted word by word (`-- echo "hello world"`).
pub fn job_command(args: &[String]) -> Option<String> {
    match args {
        [] => None,
        [one] => Some(one.clone()).filter(|c| !c.trim().is_empty()),
        many => Some(many.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")),
    }
}

pub async fn run(ctx: &Ctx, name: &str, args: &[String], follow: bool) -> Result<()> {
    let body = RunJobRequest { command: job_command(args) };
    let resp = ctx.client.post::<_, JobRun>(&["services", name, "jobs"], &[], &body).await?;
    let j = &resp.data;
    if follow {
        if !ctx.json {
            errln!("Job {} {}, streaming output…", j.id, j.status);
        }
        return follow_job(ctx, &j.id).await;
    }
    if ctx.json {
        return print_json(&resp.raw);
    }
    outln!("Job {} {}", j.id, output::out_opt(output::job_status_color(j.status), j.status.as_str()))?;
    outln!("Follow it with: ferry logs --job {} -f", j.id)?;
    Ok(())
}

pub async fn list(ctx: &Ctx, name: &str, limit: u32) -> Result<()> {
    let resp = ctx.client.get::<Vec<JobRun>>(&["services", name, "jobs"], &[("limit", limit.to_string())]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No job runs for '{name}' yet.")?;
        return Ok(());
    }
    let mut t = Table::new(&["ID", "STATUS", "TRIGGER", "COMMAND", "EXIT", "STARTED", "DURATION"]);
    for j in &resp.data {
        t.row(vec![
            Cell::new(j.id.as_str()),
            Cell::colored(j.status.as_str(), output::job_status_color(j.status)),
            Cell::new(j.trigger.as_str()),
            Cell::new(j.command.as_deref().map(|c| output::truncate(c, 40)).unwrap_or_else(|| "(default)".into())),
            Cell::new(j.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".into())),
            Cell::new(output::ago(j.started_at.or(Some(j.created_at)))),
            Cell::new(output::span(j.started_at, j.finished_at)),
        ]);
    }
    outln!("{}", t.render(output::stdout_color()).trim_end())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("manage.py"), "manage.py");
        assert_eq!(shell_quote("--flag=1"), "--flag=1");
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("*.py"), "'*.py'");
    }

    #[test]
    fn job_commands() {
        assert_eq!(job_command(&[]), None);
        assert_eq!(job_command(&args(&["  "])), None);
        assert_eq!(job_command(&args(&["npm run a && npm run b"])).as_deref(), Some("npm run a && npm run b"));
        assert_eq!(
            job_command(&args(&["python", "manage.py", "migrate", "--noinput"])).as_deref(),
            Some("python manage.py migrate --noinput")
        );
        assert_eq!(
            job_command(&args(&["echo", "hello world", "it's"])).as_deref(),
            Some(r"echo 'hello world' 'it'\''s'")
        );
        assert_eq!(job_command(&args(&["sh", "-c", "echo $X"])).as_deref(), Some("sh -c 'echo $X'"));
    }
}
