//! `ferry domains` (the server's domains) and `ferry certificates`.
//!
//! The custom domains of one service (`ferry domains NAME`, `add`, `rm`)
//! live in `services`.

use anyhow::Result;
use chrono::Utc;
use ferry_core::dto::{CertificateState, CertificateView, ConnectDomain, DnsRecord, DomainView, UpdateDomain};
use ferry_core::{CheckOutcome, DomainSource, DomainStatus};

use super::{Ctx, Exit, confirm, print_json};
use crate::output::{self, Cell, Color, Table, errln, outln};

/// `local` for a name nothing verifies, else the status.
fn status_cell(d: &DomainView) -> Cell {
    if d.local {
        return Cell::colored("local", Some(Color::Dim));
    }
    let color = match d.domain.status {
        DomainStatus::Active => Color::Green,
        DomainStatus::Pending => Color::Yellow,
        DomainStatus::Misconfigured => Color::Red,
    };
    Cell::colored(d.domain.status.as_str(), Some(color))
}

/// The DNS records to create, as an indented table.
fn records_table(records: &[DnsRecord], domain: &str) -> String {
    let mut table = Table::new(&["  TYPE", "NAME", "VALUE", ""]);
    for r in records {
        let value = r.value.as_deref().unwrap_or("<the public IP address of this server>");
        let note = if r.required { String::new() } else { format!("optional: only to serve {domain} itself") };
        table.row(vec![
            Cell::new(format!("  {}", r.record_type)),
            Cell::new(r.name.as_str()),
            Cell::new(value),
            Cell::colored(note, Some(Color::Dim)),
        ]);
    }
    table.render(output::stdout_color())
}

/// What the last verification found, one line per check.
fn print_checks(d: &DomainView) -> Result<()> {
    for check in &d.domain.checks {
        let color = match check.outcome {
            CheckOutcome::Passed => Color::Green,
            CheckOutcome::Warning => Color::Yellow,
            CheckOutcome::Failed => Color::Red,
            CheckOutcome::Skipped => Color::Dim,
        };
        // Padded before it is painted: the color codes must not count.
        let outcome = output::out(color, &format!("{:<8}", check.outcome.as_str()));
        outln!("  {:<5} {outcome} {}", check.kind.as_str(), check.message)?;
    }
    Ok(())
}

/// What to do about a domain that doesn't reach the server.
fn print_next_steps(d: &DomainView) -> Result<()> {
    let name = &d.domain.name;
    outln!("Create this DNS record where the DNS of {name} is managed:")?;
    outln!()?;
    outln!("{}", records_table(&d.records, name).trim_end())?;
    outln!()?;
    outln!("The server checks every few seconds. Check now with: ferry domains verify {name}")?;
    Ok(())
}

/// `ferry domains`: the server's domains.
pub async fn list(ctx: &Ctx) -> Result<()> {
    let resp = ctx.client.get::<Vec<DomainView>>(&["domains"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let mut table = Table::new(&["DOMAIN", "STATUS", "DEFAULT", "SERVICES AT", "SET BY"]);
    for d in &resp.data {
        let at = if d.served { d.url_pattern.clone() } else { "(waits for its DNS)".to_string() };
        table.row(vec![
            Cell::new(d.domain.name.as_str()),
            status_cell(d),
            Cell::new(if d.domain.is_default { "yes" } else { "-" }),
            Cell::new(at),
            Cell::new(match d.domain.source {
                DomainSource::Config => "--base-domain",
                DomainSource::Connected => "connected",
            }),
        ]);
    }
    outln!("{}", table.render(output::stdout_color()).trim_end())?;
    for d in resp.data.iter().filter(|d| !d.local && d.domain.status != DomainStatus::Active) {
        outln!()?;
        match d.domain.status {
            DomainStatus::Misconfigured => outln!("{} no longer reaches this server:", d.domain.name)?,
            _ => outln!("{} doesn't reach this server yet:", d.domain.name)?,
        }
        print_checks(d)?;
        print_next_steps(d)?;
    }
    if resp.data.iter().all(|d| d.domain.source == DomainSource::Config) {
        outln!()?;
        outln!("Connect a domain of yours with: ferry domains connect example.com")?;
    }
    Ok(())
}

/// `ferry domains connect DOMAIN`
pub async fn connect(ctx: &Ctx, domain: &str) -> Result<()> {
    let body = ConnectDomain { name: domain.to_string() };
    let resp = ctx.client.post::<_, DomainView>(&["domains"], &[], &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let d = &resp.data;
    let name = &d.domain.name;
    if d.served {
        outln!("Connected {name}: services are served at {}", d.url_pattern)?;
        return Ok(());
    }
    outln!("Connected {name}. Services will be served at {} once its DNS points at this server.", d.url_pattern)?;
    print_next_steps(d)
}

/// `ferry domains verify DOMAIN`: exit 1 while its names don't reach the server.
pub async fn verify(ctx: &Ctx, domain: &str) -> Result<()> {
    let resp = ctx.client.post_empty::<DomainView>(&["domains", domain, "verify"]).await?;
    let d = &resp.data;
    let reached = d.local || d.domain.status == DomainStatus::Active;
    if ctx.json {
        print_json(&resp.raw)?;
    } else {
        let name = &d.domain.name;
        if d.local {
            outln!("{name} is a local name: there is nothing to verify. Services are served at {}", d.url_pattern)?;
        } else if reached {
            outln!("{name} reaches this server: services are served at {}", d.url_pattern)?;
            print_checks(d)?;
        } else {
            outln!("{name} doesn't reach this server ({}):", d.domain.status)?;
            print_checks(d)?;
            outln!()?;
            print_next_steps(d)?;
        }
    }
    if reached { Ok(()) } else { Err(Exit(1).into()) }
}

/// `ferry domains default DOMAIN`
pub async fn set_default(ctx: &Ctx, domain: &str) -> Result<()> {
    let body = UpdateDomain { is_default: Some(true) };
    let resp = ctx.client.patch::<_, DomainView>(&["domains", domain], &[], &body).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let d = &resp.data;
    outln!("{} is the default domain: services are shown at {}", d.domain.name, d.url_pattern)?;
    errln!("Running services read their new FERRY_EXTERNAL_URL at their next deploy or restart.");
    Ok(())
}

/// `ferry domains disconnect DOMAIN`
pub async fn disconnect(ctx: &Ctx, domain: &str, yes: bool) -> Result<()> {
    // Resolve an id to the domain's name for the prompt and messages.
    let view = ctx.client.get::<DomainView>(&["domains", domain], &[]).await?.data;
    let d = &view.domain;
    confirm(&format!("disconnect '{}' (services stop being served under it)", d.name), yes).await?;
    ctx.client.delete_no_content(&["domains", &d.id], &[]).await?;
    if ctx.json {
        return print_json(&serde_json::json!({ "disconnected": d.name, "id": d.id }));
    }
    if view.local {
        outln!("Disconnected {}.", d.name)?;
    } else {
        outln!("Disconnected {}. A DNS record created for it stays at the DNS provider.", d.name)?;
    }
    Ok(())
}

fn certificate_cells(c: &CertificateView) -> (Cell, String) {
    let now = Utc::now();
    match c.state {
        CertificateState::Issued => {
            let expires = c.expires_at.map(|t| format!("expires {}", output::relative_time(t, now)));
            (Cell::colored("issued", Some(Color::Green)), expires.unwrap_or_default())
        }
        CertificateState::Issuing => (Cell::colored("issuing", Some(Color::Yellow)), "being requested".to_string()),
        CertificateState::Pending => (Cell::colored("pending", Some(Color::Yellow)), "requested shortly".to_string()),
        CertificateState::Failed => {
            let mut detail = c.error.clone().unwrap_or_default();
            if let Some(retry) = c.retry_at {
                detail = format!("{detail} (next attempt {})", output::relative_time(retry, now));
            }
            (Cell::colored("failed", Some(Color::Red)), detail)
        }
        CertificateState::Local => {
            (Cell::colored("local", Some(Color::Dim)), "no certificate for a local name".to_string())
        }
        CertificateState::Disabled => (Cell::colored("disabled", Some(Color::Dim)), String::new()),
    }
}

/// `ferry certificates`
pub async fn certificates(ctx: &Ctx) -> Result<()> {
    let resp = ctx.client.get::<Vec<CertificateView>>(&["certificates"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No hostname is routed yet: create a web service or a static site.")?;
        return Ok(());
    }
    let mut table = Table::new(&["HOST", "SERVICE", "CERTIFICATE", ""]);
    for c in &resp.data {
        let (state, detail) = certificate_cells(c);
        table.row(vec![
            Cell::new(c.host.as_str()),
            Cell::new(c.service.as_deref().unwrap_or("(dashboard)")),
            state,
            Cell::new(detail),
        ]);
    }
    outln!("{}", table.render(output::stdout_color()).trim_end())?;
    if resp.data.iter().any(|c| c.state == CertificateState::Disabled) {
        outln!()?;
        outln!("HTTPS is off on this server: start ferryd with --https-addr 0.0.0.0:443 and --acme-email <you>.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(name: &str, value: Option<&str>, required: bool) -> DnsRecord {
        DnsRecord { record_type: "A".into(), name: name.into(), value: value.map(str::to_string), required }
    }

    #[test]
    fn records_are_listed_with_what_is_optional() {
        let text = records_table(
            &[record("*", Some("203.0.113.10"), true), record("@", Some("203.0.113.10"), false)],
            "example.com",
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0].split_whitespace().collect::<Vec<_>>(), vec!["TYPE", "NAME", "VALUE"]);
        assert_eq!(lines[1], "  A      *      203.0.113.10");
        assert!(lines[2].ends_with("optional: only to serve example.com itself"), "{}", lines[2]);
        // A server that could not find its address still says which record to create.
        let text = records_table(&[record("*", None, true)], "example.com");
        assert!(text.contains("<the public IP address of this server>"), "{text}");
    }

    #[test]
    fn certificates_say_when_and_why() {
        let view = |extra: serde_json::Value| -> CertificateView {
            let mut v = json!({"host": "web.example.com", "service": "web", "state": "pending",
                "expires_at": null, "error": null, "retry_at": null});
            for (k, val) in extra.as_object().unwrap() {
                v[k] = val.clone();
            }
            serde_json::from_value(v).unwrap()
        };
        let soon = Utc::now() + chrono::Duration::days(60) + chrono::Duration::minutes(5);
        let (_, detail) = certificate_cells(&view(json!({"state": "issued", "expires_at": soon})));
        assert_eq!(detail, "expires in 2mo");
        let retry = Utc::now() + chrono::Duration::seconds(290);
        let (_, detail) =
            certificate_cells(&view(json!({"state": "failed", "error": "DNS problem", "retry_at": retry})));
        assert_eq!(detail, "DNS problem (next attempt in 4m)");
        assert_eq!(certificate_cells(&view(json!({"state": "disabled"}))).1, "");
    }
}
