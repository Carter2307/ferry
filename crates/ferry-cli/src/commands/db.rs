//! `ferry db …`: managed Postgres / Redis datastores.

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use ferry_core::dto::{CreateDatastore, DatastoreView, UpdateDatastore};
use ferry_core::{Datastore, DatastoreStatus};

use super::services::{explicit_cpus, explicit_memory};
use super::{Ctx, DefaultLimits, Exit, confirm, limit_pairs, or_dash, print_json};
use crate::cli::{DbCreateArgs, DbUpdateArgs};
use crate::client::is_unsupported_route;
use crate::output::{self, Cell, Color, Table, errln, outln};

/// How long `db create --wait` waits for the datastore to become available.
const WAIT_TIMEOUT: Duration = Duration::from_secs(180);

/// Map `ferry db create` flags to the request body (`--memory default` /
/// `--cpu 0` leave the limit to the server default).
pub fn create_request(a: &DbCreateArgs) -> CreateDatastore {
    CreateDatastore {
        name: a.name.clone(),
        kind: a.kind,
        version: a.version.clone(),
        database: a.database.clone(),
        username: a.username.clone(),
        memory_limit_mb: explicit_memory(a.memory),
        cpu_limit: explicit_cpus(a.cpu),
    }
}

/// Map `ferry db update` flags to the request body (`default` → 0 clears).
pub fn update_request(a: &DbUpdateArgs) -> UpdateDatastore {
    UpdateDatastore { memory_limit_mb: a.memory, cpu_limit: a.cpu }
}

pub async fn create(ctx: &Ctx, a: DbCreateArgs) -> Result<()> {
    let body = create_request(&a);
    let mut resp = ctx.client.post::<_, DatastoreView>(&["datastores"], &[], &body).await?;
    if a.wait && resp.data.datastore.status == DatastoreStatus::Creating {
        if !ctx.json {
            errln!("Waiting for {} '{}' to become available…", resp.data.datastore.kind, resp.data.datastore.name);
        }
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        let id = resp.data.datastore.id.clone();
        while resp.data.datastore.status == DatastoreStatus::Creating && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_secs(1)).await;
            resp = ctx.client.get::<DatastoreView>(&["datastores", &id], &[]).await?;
        }
    }
    if ctx.json {
        print_json(&resp.raw)?;
    } else {
        let d = &resp.data.datastore;
        outln!(
            "Created {} datastore '{}' ({}) — {}",
            d.kind,
            d.name,
            d.id,
            output::out_opt(output::datastore_status_color(d.status), d.status.as_str())
        )?;
        match d.status {
            DatastoreStatus::Available => outln!("Internal URL: {}", resp.data.internal_url)?,
            DatastoreStatus::Creating => outln!("Check progress with: ferry db show {}", d.name)?,
            DatastoreStatus::Failed => {}
        }
    }
    if resp.data.datastore.status == DatastoreStatus::Failed {
        if !ctx.json {
            errln!("error: provisioning failed: {}", or_dash(resp.data.datastore.error.as_deref()));
        }
        return Err(Exit(1).into());
    }
    Ok(())
}

pub async fn list(ctx: &Ctx) -> Result<()> {
    let resp = ctx.client.get::<Vec<DatastoreView>>(&["datastores"], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    if resp.data.is_empty() {
        outln!("No datastores yet. Create one with: ferry db create NAME [--kind postgres|redis]")?;
        return Ok(());
    }
    let mut t = Table::new(&["NAME", "KIND", "VERSION", "STATUS", "INTERNAL", "HOST PORT", "CREATED"]);
    for v in &resp.data {
        let d = &v.datastore;
        t.row(vec![
            Cell::new(d.name.as_str()),
            Cell::new(d.kind.as_str()),
            Cell::new(d.version.as_str()),
            Cell::colored(d.status.as_str(), output::datastore_status_color(d.status)),
            Cell::new(format!("{}:{}", v.internal_host, v.internal_port)),
            Cell::new(d.host_port.map(|p| p.to_string()).unwrap_or_else(|| "-".into())),
            Cell::new(output::relative_time(d.created_at, Utc::now())),
        ]);
    }
    outln!("{}", t.render(output::stdout_color()).trim_end())?;
    Ok(())
}

/// `ferry db update NAME --memory/--cpu`: new limits, applied to the running
/// container in place (no restart, no data loss).
pub async fn update(ctx: &Ctx, a: DbUpdateArgs) -> Result<()> {
    let body = update_request(&a);
    let resp = match ctx.client.patch::<_, DatastoreView>(&["datastores", &a.name], &[], &body).await {
        Ok(r) => r,
        Err(e) if is_unsupported_route(&e) => {
            return Err(e.context("this Ferry server can't change datastore limits: upgrade ferryd"));
        }
        Err(e) => return Err(e),
    };
    if ctx.json {
        return print_json(&resp.raw);
    }
    let d = &resp.data.datastore;
    let defaults = DefaultLimits::fetch_if_unset(ctx, d.memory_limit_mb, d.cpu_limit).await;
    outln!("Updated datastore '{}': {}", d.name, limits_summary(&a, d, defaults))?;
    match d.status {
        DatastoreStatus::Available => outln!("Applied to the running container (no restart).")?,
        DatastoreStatus::Creating => outln!("It starts with these limits once provisioned.")?,
        DatastoreStatus::Failed => {}
    }
    Ok(())
}

/// `memory limit 1 GiB, CPU limit server default (1 CPU)`: the limits the
/// command changed, as the server now has them.
fn limits_summary(a: &DbUpdateArgs, d: &Datastore, defaults: DefaultLimits) -> String {
    let mut parts = Vec::new();
    if a.memory.is_some() {
        parts.push(format!("memory limit {}", output::memory_limit(d.memory_limit_mb, defaults.memory_mb)));
    }
    if a.cpu.is_some() {
        parts.push(format!("CPU limit {}", output::cpu_limit(d.cpu_limit, defaults.cpus)));
    }
    parts.join(", ")
}

pub async fn show(ctx: &Ctx, name: &str) -> Result<()> {
    let resp = ctx.client.get::<DatastoreView>(&["datastores", name], &[]).await?;
    if ctx.json {
        return print_json(&resp.raw);
    }
    let v = &resp.data;
    let d = &v.datastore;
    let mut pairs: Vec<(&str, Cell)> = vec![
        ("Name", Cell::new(d.name.as_str())),
        ("ID", Cell::new(d.id.as_str())),
        ("Kind", Cell::new(format!("{} {}", d.kind, d.version))),
        ("Status", Cell::colored(d.status.as_str(), output::datastore_status_color(d.status))),
    ];
    if let Some(e) = &d.error {
        pairs.push(("Error", Cell::colored(e.as_str(), Some(Color::Red))));
    }
    if let Some(db) = &d.database {
        pairs.push(("Database", Cell::new(db.as_str())));
    }
    pairs.push(("User", Cell::new(d.username.as_str())));
    pairs.push(("Password", Cell::new(d.password.as_str())));
    pairs.push(("Internal host", Cell::new(format!("{}:{}", v.internal_host, v.internal_port))));
    pairs.push(("Internal URL", Cell::new(v.internal_url.as_str())));
    pairs.push(("External URL", Cell::new(or_dash(v.external_url.as_deref()))));
    let defaults = DefaultLimits::fetch_if_unset(ctx, d.memory_limit_mb, d.cpu_limit).await;
    pairs.extend(limit_pairs(d.memory_limit_mb, d.cpu_limit, defaults));
    pairs.push(("Created", Cell::new(output::relative_time(d.created_at, Utc::now()))));
    outln!("{}", output::render_kv(&pairs, output::stdout_color()).trim_end())?;
    outln!()?;
    outln!("Use the internal URL from services (e.g. ferry env set APP DATABASE_URL='{}'),", reference(d))?;
    outln!("and the external URL from this machine.")?;
    Ok(())
}

/// Env var reference resolved at container start (DESIGN.md §11).
fn reference(d: &ferry_core::Datastore) -> String {
    format!("${{{{datastore.{}.connectionString}}}}", d.name)
}

pub async fn remove(ctx: &Ctx, name: &str, yes: bool, force: bool) -> Result<()> {
    // Resolve an id to the datastore's real name for the prompt and messages.
    let ds = ctx.client.get::<DatastoreView>(&["datastores", name], &[]).await?.data.datastore;
    confirm(&format!("delete datastore '{}' and all its data", ds.name), yes).await?;
    ctx.client
        .delete_no_content(&["datastores", &ds.id], &super::force_query(force, false))
        .await
        .map_err(|e| super::with_force_hint(e, force))?;
    if ctx.json {
        return print_json(&serde_json::json!({ "deleted": ds.name, "id": ds.id }));
    }
    outln!("Deleted datastore '{}'", ds.name)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command, DbCommand};
    use clap::Parser;
    use ferry_core::DatastoreKind;

    fn parse(args: &[&str]) -> DbCommand {
        match Cli::try_parse_from(std::iter::once("ferry").chain(args.iter().copied())).unwrap().command {
            Command::Db(c) => c,
            other => panic!("not a db command: {other:?}"),
        }
    }

    #[test]
    fn limit_flags_map_to_requests() {
        let DbCommand::Create(a) = parse(&["db", "create", "main", "--memory", "256M", "--cpu", "0.5"]) else {
            panic!()
        };
        let body = create_request(&a);
        assert_eq!((body.memory_limit_mb, body.cpu_limit), (Some(256), Some(0.5)));
        let DbCommand::Create(a) = parse(&["db", "create", "main", "--memory", "default"]) else { panic!() };
        assert_eq!((create_request(&a).memory_limit_mb, create_request(&a).cpu_limit), (None, None));

        let DbCommand::Update(a) = parse(&["db", "update", "main", "--memory", "1G"]) else { panic!() };
        let json = serde_json::to_value(update_request(&a)).unwrap();
        assert_eq!(json, serde_json::json!({ "memory_limit_mb": 1024, "cpu_limit": null }));
        let DbCommand::Update(a) = parse(&["db", "update", "main", "--memory", "default", "--cpu", "default"]) else {
            panic!()
        };
        let json = serde_json::to_value(update_request(&a)).unwrap();
        assert_eq!(json, serde_json::json!({ "memory_limit_mb": 0, "cpu_limit": 0.0 }));
    }

    #[test]
    fn update_summary_names_what_changed() {
        let DbCommand::Update(a) = parse(&["db", "update", "main", "--cpu", "default"]) else { panic!() };
        let mut d = Datastore::new("main", DatastoreKind::Postgres);
        d.memory_limit_mb = Some(1024);
        let defaults = DefaultLimits { memory_mb: Some(512), cpus: Some(1.0) };
        assert_eq!(limits_summary(&a, &d, defaults), "CPU limit server default (1 CPU)");
        let DbCommand::Update(a) = parse(&["db", "update", "main", "--cpu", "2", "--memory", "1G"]) else { panic!() };
        d.cpu_limit = Some(2.0);
        assert_eq!(limits_summary(&a, &d, DefaultLimits::default()), "memory limit 1 GiB, CPU limit 2 CPUs");
    }

    #[test]
    fn env_reference_syntax() {
        let d = Datastore::new("app-db", DatastoreKind::Postgres);
        assert_eq!(reference(&d), "${{datastore.app-db.connectionString}}");
    }
}
