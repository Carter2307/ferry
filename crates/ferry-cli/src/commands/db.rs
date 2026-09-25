//! `ferry db …`: managed Postgres / Redis datastores.

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use ferry_core::DatastoreStatus;
use ferry_core::dto::{CreateDatastore, DatastoreView};

use super::{Ctx, Exit, confirm, or_dash, print_json};
use crate::cli::DbCreateArgs;
use crate::output::{self, Cell, Color, Table, errln, outln};

/// How long `db create --wait` waits for the datastore to become available.
const WAIT_TIMEOUT: Duration = Duration::from_secs(180);

pub async fn create(ctx: &Ctx, a: DbCreateArgs) -> Result<()> {
    let body = CreateDatastore {
        name: a.name.clone(),
        kind: a.kind,
        version: a.version.clone(),
        database: a.database.clone(),
        username: a.username.clone(),
    };
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

pub async fn remove(ctx: &Ctx, name: &str, yes: bool) -> Result<()> {
    confirm(&format!("delete datastore '{name}' and all its data"), yes).await?;
    ctx.client.delete_no_content(&["datastores", name]).await?;
    if ctx.json {
        return print_json(&serde_json::json!({ "deleted": name }));
    }
    outln!("Deleted datastore '{name}'")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::{Datastore, DatastoreKind};

    #[test]
    fn env_reference_syntax() {
        let d = Datastore::new("app-db", DatastoreKind::Postgres);
        assert_eq!(reference(&d), "${{datastore.app-db.connectionString}}");
    }
}
