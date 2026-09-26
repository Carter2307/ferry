//! `ferry logs`: runtime logs of a service, or the log of a deploy / job.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ferry_core::dto::{RuntimeStatus, ServiceView};
use ferry_core::{ServiceState, ServiceType};

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

/// Why a service has no runtime output, and where to look instead.
pub fn no_instances_hint(v: &ServiceView, follow: bool) -> String {
    let n = &v.service.name;
    let mut hint = if v.service.service_type == ServiceType::CronJob {
        return format!(
            "'{n}' is a cron job: it has no long-running instances, each run has its own log. \
             List runs with 'ferry jobs {n}', then use 'ferry logs --job JOB_ID'"
        );
    } else if v.state == ServiceState::Suspended {
        format!("'{n}' is suspended, so nothing is running. Resume it with 'ferry resume {n}'")
    } else {
        match &v.latest_deploy {
            None => format!("'{n}' has no running instances: it has never been deployed"),
            Some(d) if d.status.is_active() => format!(
                "'{n}' has no running instances yet: deploy {} is {}. Follow it with 'ferry logs --deploy {} -f'",
                d.id, d.status, d.id
            ),
            Some(d) if d.status.is_failed() => {
                let reason = d.error.as_deref().map(|e| format!(": {}", output::truncate(e, 160))).unwrap_or_default();
                format!(
                    "'{n}' has no running instances: its latest deploy {} failed ({}{reason}). \
                     See its log with 'ferry logs --deploy {}'",
                    d.id, d.status, d.id
                )
            }
            Some(_) => format!(
                "'{n}' has no running instances (state: {}). Check 'ferry status {n}' and 'ferry deploys {n}'",
                v.state
            ),
        }
    };
    if follow {
        hint.push_str(". Waiting for new instances…");
    }
    hint
}

/// Explain up front when nothing is running, instead of printing nothing
/// (or waiting silently). Best effort: errors surface from the log stream.
async fn explain_if_idle(ctx: &Ctx, name: &str, follow: bool) {
    let Ok(status) = ctx.client.get::<RuntimeStatus>(&["services", name, "status"], &[]).await else { return };
    if status.data.instances.iter().any(|i| i.state == "running") {
        return;
    }
    let Ok(view) = ctx.client.get::<ServiceView>(&["services", name], &[]).await else { return };
    errln!("{}", output::err(Color::Yellow, &no_instances_hint(&view.data, follow)));
}

/// Runtime logs. With `follow`, keep going across redeploys and dropped
/// connections: when a stream ends, reconnect for new lines only (`tail=0`).
async fn runtime(ctx: &Ctx, name: &str, follow: bool, tail: Option<u32>) -> Result<()> {
    explain_if_idle(ctx, name, follow).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{Deploy, DeploySource, DeployStatus, DeployTrigger, Service};

    fn view(t: ServiceType, state: ServiceState, latest: Option<(DeployStatus, Option<&str>)>) -> ServiceView {
        let mut service = Service::new("unknown", t);
        service.id = "srv-unknown".into();
        let latest_deploy = latest.map(|(status, error)| {
            let mut d = Deploy::new("srv-unknown", DeployTrigger::Upload, DeploySource::Archive { path: "p".into() });
            d.id = "dep-1".into();
            d.status = status;
            d.error = error.map(str::to_string);
            d
        });
        ServiceView {
            deploy_hook_path: String::new(),
            url: None,
            hosts: vec![],
            internal_host: "unknown".into(),
            internal_port: None,
            env_groups: vec![],
            latest_deploy,
            state,
            service,
        }
    }

    #[test]
    fn hints_explain_why_nothing_runs() {
        let failed = view(
            ServiceType::WebService,
            ServiceState::Failed,
            Some((DeployStatus::BuildFailed, Some("cannot determine how to start this app"))),
        );
        let h = no_instances_hint(&failed, false);
        assert!(h.contains("latest deploy dep-1 failed (build_failed: cannot determine how to start"), "{h}");
        assert!(h.ends_with("See its log with 'ferry logs --deploy dep-1'"), "{h}");
        assert!(no_instances_hint(&failed, true).ends_with("Waiting for new instances…"));

        let building = view(ServiceType::WebService, ServiceState::Deploying, Some((DeployStatus::Building, None)));
        let h = no_instances_hint(&building, false);
        assert!(h.contains("deploy dep-1 is building") && h.contains("ferry logs --deploy dep-1 -f"), "{h}");

        let never = view(ServiceType::BackgroundWorker, ServiceState::NotDeployed, None);
        assert!(no_instances_hint(&never, false).contains("never been deployed"));

        let suspended = view(ServiceType::WebService, ServiceState::Suspended, Some((DeployStatus::Live, None)));
        assert!(no_instances_hint(&suspended, false).contains("ferry resume unknown"));

        let crashed = view(ServiceType::WebService, ServiceState::Degraded, Some((DeployStatus::Live, None)));
        assert!(no_instances_hint(&crashed, false).contains("(state: degraded)"));

        let cron = view(ServiceType::CronJob, ServiceState::Live, Some((DeployStatus::Live, None)));
        let h = no_instances_hint(&cron, true);
        assert!(h.contains("cron job") && h.contains("ferry jobs unknown") && !h.contains("Waiting"), "{h}");
    }
}
