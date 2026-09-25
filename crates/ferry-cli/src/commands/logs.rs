//! `ferry logs`: runtime logs of a service, or the log of a deploy / job.

use std::time::Duration;

use anyhow::{Result, anyhow};

use super::{Ctx, follow_deploy, follow_job, print_log_event};
use crate::cli::LogsArgs;
use crate::client::ApiError;
use crate::output::{self, Color, errln};

const MAX_RECONNECTS: u32 = 5;

pub async fn logs(ctx: &Ctx, a: LogsArgs) -> Result<()> {
    if let Some(id) = &a.deploy {
        return if a.follow { follow_deploy(ctx, id).await } else { replay(ctx, &["deploys", id, "logs"]).await };
    }
    if let Some(id) = &a.job {
        return if a.follow { follow_job(ctx, id).await } else { replay(ctx, &["jobs", id, "logs"]).await };
    }
    let name = a.name.as_deref().ok_or_else(|| anyhow!("pass a service name, --deploy ID or --job ID"))?;
    runtime(ctx, name, a.follow, a.tail).await
}

/// Print a stored deploy/job log once (`follow=false`).
async fn replay(ctx: &Ctx, segments: &[&str]) -> Result<()> {
    let mut stream = ctx.client.events(segments, &[("follow", "false".to_string())]).await?;
    while let Some(ev) = stream.next(None).await? {
        match ev.event.as_str() {
            "log" => print_log_event(ctx, &ev.data)?,
            "end" => break,
            "error" => errln!("{} {}", output::err(Color::Red, "server:"), ev.data),
            _ => {}
        }
    }
    Ok(())
}

/// Runtime logs. With `follow`, keep going across redeploys and dropped
/// connections: when a stream ends, reconnect for new lines only (`tail=0`).
async fn runtime(ctx: &Ctx, name: &str, follow: bool, tail: Option<u32>) -> Result<()> {
    let segments = ["services", name, "logs"];
    let mut tail = tail;
    let mut failures = 0u32;
    loop {
        let mut query = vec![("follow", follow.to_string())];
        if let Some(t) = tail {
            query.push(("tail", t.to_string()));
        }
        let problem = match ctx.client.events(&segments, &query).await {
            Ok(mut stream) => loop {
                match stream.next(None).await {
                    Ok(Some(ev)) => match ev.event.as_str() {
                        "log" => {
                            print_log_event(ctx, &ev.data)?;
                            failures = 0;
                        }
                        "end" => break None,
                        "error" => errln!("{} {}", output::err(Color::Red, "server:"), ev.data),
                        _ => {}
                    },
                    Ok(None) => break None,
                    Err(e) => break Some(e),
                }
            },
            Err(e) if !follow || e.downcast_ref::<ApiError>().is_some_and(|a| a.status < 500) => return Err(e),
            Err(e) => Some(e),
        };
        if !follow {
            return problem.map_or(Ok(()), Err);
        }
        let delay = match problem {
            None => Duration::from_secs(2),
            Some(e) => {
                failures += 1;
                if failures > MAX_RECONNECTS {
                    return Err(e.context("log stream lost"));
                }
                errln!("{}", output::err(Color::Dim, &format!("({e:#}; reconnecting…)")));
                Duration::from_millis(500 * 2u64.pow(failures.min(4)))
            }
        };
        tail = Some(0);
        tokio::time::sleep(delay).await;
    }
}
