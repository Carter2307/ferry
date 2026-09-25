//! SQLite persistence. [`Store`] is cheap to clone (it wraps a connection pool)
//! and safe to share between the API and the engine.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

use crate::config::Config;
use crate::env::{self, ServiceRef};
use crate::models::*;
use crate::{Error, Result};

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// Handle to the Ferry database.
#[derive(Clone, Debug)]
pub struct Store {
    pool: SqlitePool,
}

// ---------------------------------------------------------------------------
// encoding helpers

fn ts(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn ts_opt(dt: &Option<DateTime<Utc>>) -> Option<String> {
    dt.as_ref().map(ts)
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| Error::internal(format!("bad timestamp '{s}' in database: {e}")))
}

fn get_ts(r: &SqliteRow, col: &str) -> Result<DateTime<Utc>> {
    parse_ts(&r.try_get::<String, _>(col)?)
}

fn get_ts_opt(r: &SqliteRow, col: &str) -> Result<Option<DateTime<Utc>>> {
    r.try_get::<Option<String>, _>(col)?.map(|s| parse_ts(&s)).transpose()
}

fn get_enum<T: FromStr<Err = Error>>(r: &SqliteRow, col: &str) -> Result<T> {
    r.try_get::<String, _>(col)?.parse()
}

fn get_u16_opt(r: &SqliteRow, col: &str) -> Result<Option<u16>> {
    Ok(r.try_get::<Option<i64>, _>(col)?.and_then(|v| u16::try_from(v).ok()))
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}

fn row_to_service(r: &SqliteRow) -> Result<Service> {
    let domains: String = r.try_get("custom_domains")?;
    Ok(Service {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        service_type: get_enum(r, "service_type")?,
        repo_url: r.try_get("repo_url")?,
        branch: r.try_get("branch")?,
        image: r.try_get("image")?,
        runtime: get_enum(r, "runtime")?,
        root_dir: r.try_get("root_dir")?,
        dockerfile_path: r.try_get("dockerfile_path")?,
        build_command: r.try_get("build_command")?,
        start_command: r.try_get("start_command")?,
        publish_dir: r.try_get("publish_dir")?,
        port: get_u16_opt(r, "port")?,
        health_check_path: r.try_get("health_check_path")?,
        schedule: r.try_get("schedule")?,
        instances: r.try_get::<i64, _>("instances")?.clamp(0, u32::MAX as i64) as u32,
        auto_deploy: r.try_get("auto_deploy")?,
        suspended: r.try_get("suspended")?,
        disk_mount_path: r.try_get("disk_mount_path")?,
        custom_domains: serde_json::from_str(&domains)
            .map_err(|e| Error::internal(format!("bad custom_domains json: {e}")))?,
        deploy_hook_key: r.try_get("deploy_hook_key")?,
        live_deploy_id: r.try_get("live_deploy_id")?,
        created_at: get_ts(r, "created_at")?,
        updated_at: get_ts(r, "updated_at")?,
    })
}

fn row_to_deploy(r: &SqliteRow) -> Result<Deploy> {
    let source: String = r.try_get("source")?;
    Ok(Deploy {
        id: r.try_get("id")?,
        service_id: r.try_get("service_id")?,
        status: get_enum(r, "status")?,
        trigger: get_enum(r, "trigger_kind")?,
        source: serde_json::from_str(&source).map_err(|e| Error::internal(format!("bad deploy source json: {e}")))?,
        commit_sha: r.try_get("commit_sha")?,
        commit_message: r.try_get("commit_message")?,
        image: r.try_get("image")?,
        port: get_u16_opt(r, "port")?,
        error: r.try_get("error")?,
        created_at: get_ts(r, "created_at")?,
        started_at: get_ts_opt(r, "started_at")?,
        finished_at: get_ts_opt(r, "finished_at")?,
    })
}

fn row_to_job(r: &SqliteRow) -> Result<JobRun> {
    Ok(JobRun {
        id: r.try_get("id")?,
        service_id: r.try_get("service_id")?,
        trigger: get_enum(r, "trigger_kind")?,
        command: r.try_get("command")?,
        image: r.try_get("image")?,
        status: get_enum(r, "status")?,
        exit_code: r.try_get("exit_code")?,
        error: r.try_get("error")?,
        created_at: get_ts(r, "created_at")?,
        started_at: get_ts_opt(r, "started_at")?,
        finished_at: get_ts_opt(r, "finished_at")?,
    })
}

fn row_to_datastore(r: &SqliteRow) -> Result<Datastore> {
    Ok(Datastore {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        kind: get_enum(r, "kind")?,
        version: r.try_get("version")?,
        status: get_enum(r, "status")?,
        username: r.try_get("username")?,
        password: r.try_get("password")?,
        database: r.try_get("database")?,
        host_port: get_u16_opt(r, "host_port")?,
        error: r.try_get("error")?,
        created_at: get_ts(r, "created_at")?,
        updated_at: get_ts(r, "updated_at")?,
    })
}

fn row_to_env_group(r: &SqliteRow) -> Result<EnvGroup> {
    Ok(EnvGroup {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        created_at: get_ts(r, "created_at")?,
        updated_at: get_ts(r, "updated_at")?,
    })
}

fn row_to_env(r: &SqliteRow) -> Result<EnvVar> {
    Ok(EnvVar { key: r.try_get("key")?, value: r.try_get("value")? })
}

impl Store {
    // -----------------------------------------------------------------------
    // lifecycle

    /// Open (creating if needed) the database at `path` and run migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new().max_connections(8).connect_with(opts).await?;
        let store = Store { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// A private in-memory database (for tests).
    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        // One connection: every in-memory connection would be its own database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await?;
        let store = Store { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        let version: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&self.pool).await?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
            let mut tx = self.pool.begin().await?;
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            sqlx::query(&format!("PRAGMA user_version = {}", i + 1)).execute(&mut *tx).await?;
            tx.commit().await?;
            tracing::info!(version = i + 1, "applied database migration");
        }
        Ok(())
    }

    /// Raw pool access (escape hatch).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // -----------------------------------------------------------------------
    // names

    /// True if a service or datastore already uses `name` (they share the
    /// private-network hostname namespace).
    pub async fn name_taken(&self, name: &str) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM services WHERE name = ?1) + (SELECT COUNT(*) FROM datastores WHERE name = ?1)",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    // -----------------------------------------------------------------------
    // services

    /// Insert a new service. Conflict if the name is taken by a service or datastore.
    pub async fn create_service(&self, s: &Service) -> Result<()> {
        if self.name_taken(&s.name).await? {
            return Err(Error::conflict(format!("name '{}' is already in use", s.name)));
        }
        let res = sqlx::query(
            "INSERT INTO services (id, name, service_type, repo_url, branch, image, runtime, root_dir, dockerfile_path,
                build_command, start_command, publish_dir, port, health_check_path, schedule, instances, auto_deploy,
                suspended, disk_mount_path, custom_domains, deploy_hook_key, live_deploy_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(s.service_type.as_str())
        .bind(&s.repo_url)
        .bind(&s.branch)
        .bind(&s.image)
        .bind(s.runtime.as_str())
        .bind(&s.root_dir)
        .bind(&s.dockerfile_path)
        .bind(&s.build_command)
        .bind(&s.start_command)
        .bind(&s.publish_dir)
        .bind(s.port.map(i64::from))
        .bind(&s.health_check_path)
        .bind(&s.schedule)
        .bind(i64::from(s.instances))
        .bind(s.auto_deploy)
        .bind(s.suspended)
        .bind(&s.disk_mount_path)
        .bind(serde_json::to_string(&s.custom_domains).unwrap_or_else(|_| "[]".into()))
        .bind(&s.deploy_hook_key)
        .bind(&s.live_deploy_id)
        .bind(ts(&s.created_at))
        .bind(ts(&s.updated_at))
        .execute(&self.pool)
        .await;
        match res {
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => Err(Error::conflict(format!("name '{}' is already in use", s.name))),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist every mutable column of `s` (name and id are immutable) and
    /// bump `updated_at`. Returns the stored row.
    pub async fn update_service(&self, s: &Service) -> Result<Service> {
        let res = sqlx::query(
            "UPDATE services SET service_type = ?, repo_url = ?, branch = ?, image = ?, runtime = ?, root_dir = ?,
                dockerfile_path = ?, build_command = ?, start_command = ?, publish_dir = ?, port = ?,
                health_check_path = ?, schedule = ?, instances = ?, auto_deploy = ?, suspended = ?,
                disk_mount_path = ?, custom_domains = ?, deploy_hook_key = ?, live_deploy_id = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(s.service_type.as_str())
        .bind(&s.repo_url)
        .bind(&s.branch)
        .bind(&s.image)
        .bind(s.runtime.as_str())
        .bind(&s.root_dir)
        .bind(&s.dockerfile_path)
        .bind(&s.build_command)
        .bind(&s.start_command)
        .bind(&s.publish_dir)
        .bind(s.port.map(i64::from))
        .bind(&s.health_check_path)
        .bind(&s.schedule)
        .bind(i64::from(s.instances))
        .bind(s.auto_deploy)
        .bind(s.suspended)
        .bind(&s.disk_mount_path)
        .bind(serde_json::to_string(&s.custom_domains).unwrap_or_else(|_| "[]".into()))
        .bind(&s.deploy_hook_key)
        .bind(&s.live_deploy_id)
        .bind(ts(&Utc::now()))
        .bind(&s.id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("service", &s.id));
        }
        self.require_service(&s.id).await
    }

    pub async fn get_service(&self, id: &str) -> Result<Option<Service>> {
        sqlx::query("SELECT * FROM services WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| row_to_service(&r))
            .transpose()
    }

    /// Look up by id or name.
    pub async fn find_service(&self, id_or_name: &str) -> Result<Option<Service>> {
        sqlx::query("SELECT * FROM services WHERE id = ?1 OR name = ?1 LIMIT 1")
            .bind(id_or_name)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| row_to_service(&r))
            .transpose()
    }

    /// Look up by id or name; NotFound if missing.
    pub async fn require_service(&self, id_or_name: &str) -> Result<Service> {
        self.find_service(id_or_name).await?.ok_or_else(|| Error::not_found("service", id_or_name))
    }

    /// All services ordered by name.
    pub async fn list_services(&self) -> Result<Vec<Service>> {
        sqlx::query("SELECT * FROM services ORDER BY name")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_service)
            .collect()
    }

    /// Delete a service with its env vars, env group links, deploys and job runs.
    pub async fn delete_service(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM env_vars WHERE owner_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM service_env_groups WHERE service_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM deploys WHERE service_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM job_runs WHERE service_id = ?").bind(id).execute(&mut *tx).await?;
        let res = sqlx::query("DELETE FROM services WHERE id = ?").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("service", id));
        }
        Ok(())
    }

    pub async fn set_live_deploy(&self, service_id: &str, deploy_id: Option<&str>) -> Result<()> {
        self.exec_service_update("live_deploy_id = ?", service_id, deploy_id.map(str::to_string)).await
    }

    pub async fn set_suspended(&self, service_id: &str, suspended: bool) -> Result<()> {
        let res = sqlx::query("UPDATE services SET suspended = ?, updated_at = ? WHERE id = ?")
            .bind(suspended)
            .bind(ts(&Utc::now()))
            .bind(service_id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("service", service_id));
        }
        Ok(())
    }

    pub async fn set_instances(&self, service_id: &str, instances: u32) -> Result<()> {
        let res = sqlx::query("UPDATE services SET instances = ?, updated_at = ? WHERE id = ?")
            .bind(i64::from(instances))
            .bind(ts(&Utc::now()))
            .bind(service_id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("service", service_id));
        }
        Ok(())
    }

    async fn exec_service_update(&self, set_clause: &str, service_id: &str, value: Option<String>) -> Result<()> {
        let sql = format!("UPDATE services SET {set_clause}, updated_at = ? WHERE id = ?");
        let res = sqlx::query(&sql).bind(value).bind(ts(&Utc::now())).bind(service_id).execute(&self.pool).await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("service", service_id));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // env vars (owner = service id or env group id)

    /// Variables of an owner, sorted by key.
    pub async fn list_env(&self, owner_id: &str) -> Result<Vec<EnvVar>> {
        sqlx::query("SELECT key, value FROM env_vars WHERE owner_id = ? ORDER BY key")
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_env)
            .collect()
    }

    /// Insert or update one variable.
    pub async fn set_env(&self, owner_id: &str, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO env_vars (owner_id, key, value) VALUES (?, ?, ?)
             ON CONFLICT(owner_id, key) DO UPDATE SET value = excluded.value",
        )
        .bind(owner_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete one variable; returns whether it existed.
    pub async fn unset_env(&self, owner_id: &str, key: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM env_vars WHERE owner_id = ? AND key = ?")
            .bind(owner_id)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Replace all variables of an owner atomically.
    pub async fn replace_env(&self, owner_id: &str, vars: &[EnvVar]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM env_vars WHERE owner_id = ?").bind(owner_id).execute(&mut *tx).await?;
        for v in vars {
            sqlx::query(
                "INSERT INTO env_vars (owner_id, key, value) VALUES (?, ?, ?)
                 ON CONFLICT(owner_id, key) DO UPDATE SET value = excluded.value",
            )
            .bind(owner_id)
            .bind(&v.key)
            .bind(&v.value)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Upsert `set` and delete `unset` atomically.
    pub async fn patch_env(&self, owner_id: &str, set: &[EnvVar], unset: &[String]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for k in unset {
            sqlx::query("DELETE FROM env_vars WHERE owner_id = ? AND key = ?")
                .bind(owner_id)
                .bind(k)
                .execute(&mut *tx)
                .await?;
        }
        for v in set {
            sqlx::query(
                "INSERT INTO env_vars (owner_id, key, value) VALUES (?, ?, ?)
                 ON CONFLICT(owner_id, key) DO UPDATE SET value = excluded.value",
            )
            .bind(owner_id)
            .bind(&v.key)
            .bind(&v.value)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The service's unresolved environment: linked env groups (in link
    /// order) overridden by the service's own variables.
    pub async fn effective_env(&self, service_id: &str) -> Result<Vec<EnvVar>> {
        let mut layers = Vec::new();
        for g in self.service_env_groups(service_id).await? {
            layers.push(self.list_env(&g.id).await?);
        }
        layers.push(self.list_env(service_id).await?);
        Ok(env::merge(&layers))
    }

    // -----------------------------------------------------------------------
    // env groups

    pub async fn create_env_group(&self, g: &EnvGroup) -> Result<()> {
        let res = sqlx::query("INSERT INTO env_groups (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(&g.id)
            .bind(&g.name)
            .bind(ts(&g.created_at))
            .bind(ts(&g.updated_at))
            .execute(&self.pool)
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => Err(Error::conflict(format!("env group '{}' already exists", g.name))),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn find_env_group(&self, id_or_name: &str) -> Result<Option<EnvGroup>> {
        sqlx::query("SELECT * FROM env_groups WHERE id = ?1 OR name = ?1 LIMIT 1")
            .bind(id_or_name)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| row_to_env_group(&r))
            .transpose()
    }

    pub async fn require_env_group(&self, id_or_name: &str) -> Result<EnvGroup> {
        self.find_env_group(id_or_name).await?.ok_or_else(|| Error::not_found("env group", id_or_name))
    }

    pub async fn list_env_groups(&self) -> Result<Vec<EnvGroup>> {
        sqlx::query("SELECT * FROM env_groups ORDER BY name")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_env_group)
            .collect()
    }

    /// Bump an env group's `updated_at` (call after changing its variables).
    pub async fn touch_env_group(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE env_groups SET updated_at = ? WHERE id = ?")
            .bind(ts(&Utc::now()))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete an env group, its variables and its service links.
    pub async fn delete_env_group(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM env_vars WHERE owner_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM service_env_groups WHERE group_id = ?").bind(id).execute(&mut *tx).await?;
        let res = sqlx::query("DELETE FROM env_groups WHERE id = ?").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("env group", id));
        }
        Ok(())
    }

    /// Link an env group to a service (idempotent; appended last).
    pub async fn link_env_group(&self, service_id: &str, group_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO service_env_groups (service_id, group_id, position)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM service_env_groups WHERE service_id = ?1))",
        )
        .bind(service_id)
        .bind(group_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Unlink; returns whether a link existed.
    pub async fn unlink_env_group(&self, service_id: &str, group_id: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM service_env_groups WHERE service_id = ? AND group_id = ?")
            .bind(service_id)
            .bind(group_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Env groups linked to a service, in link order.
    pub async fn service_env_groups(&self, service_id: &str) -> Result<Vec<EnvGroup>> {
        sqlx::query(
            "SELECT g.* FROM env_groups g JOIN service_env_groups l ON l.group_id = g.id
             WHERE l.service_id = ? ORDER BY l.position",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(row_to_env_group)
        .collect()
    }

    /// Services linked to an env group (ordered by name).
    pub async fn env_group_services(&self, group_id: &str) -> Result<Vec<Service>> {
        sqlx::query(
            "SELECT s.* FROM services s JOIN service_env_groups l ON l.service_id = s.id
             WHERE l.group_id = ? ORDER BY s.name",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(row_to_service)
        .collect()
    }

    // -----------------------------------------------------------------------
    // deploys

    pub async fn create_deploy(&self, d: &Deploy) -> Result<()> {
        sqlx::query(
            "INSERT INTO deploys (id, service_id, status, trigger_kind, source, commit_sha, commit_message, image,
                port, error, created_at, started_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&d.id)
        .bind(&d.service_id)
        .bind(d.status.as_str())
        .bind(d.trigger.as_str())
        .bind(serde_json::to_string(&d.source).map_err(|e| Error::internal(e.to_string()))?)
        .bind(&d.commit_sha)
        .bind(&d.commit_message)
        .bind(&d.image)
        .bind(d.port.map(i64::from))
        .bind(&d.error)
        .bind(ts(&d.created_at))
        .bind(ts_opt(&d.started_at))
        .bind(ts_opt(&d.finished_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persist every mutable column of a deploy.
    pub async fn update_deploy(&self, d: &Deploy) -> Result<()> {
        let res = sqlx::query(
            "UPDATE deploys SET status = ?, source = ?, commit_sha = ?, commit_message = ?, image = ?, port = ?,
                error = ?, started_at = ?, finished_at = ?
             WHERE id = ?",
        )
        .bind(d.status.as_str())
        .bind(serde_json::to_string(&d.source).map_err(|e| Error::internal(e.to_string()))?)
        .bind(&d.commit_sha)
        .bind(&d.commit_message)
        .bind(&d.image)
        .bind(d.port.map(i64::from))
        .bind(&d.error)
        .bind(ts_opt(&d.started_at))
        .bind(ts_opt(&d.finished_at))
        .bind(&d.id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("deploy", &d.id));
        }
        Ok(())
    }

    /// Change a deploy's status, stamping `started_at` when it starts building
    /// or deploying and `finished_at` when it reaches a terminal status
    /// (each only if not already set). `error` replaces the stored error when
    /// `Some`. Returns the updated deploy.
    pub async fn set_deploy_status(&self, id: &str, status: DeployStatus, error: Option<&str>) -> Result<Deploy> {
        let mut d = self.require_deploy(id).await?;
        let now = Utc::now();
        d.status = status;
        if matches!(status, DeployStatus::Building | DeployStatus::Deploying) && d.started_at.is_none() {
            d.started_at = Some(now);
        }
        if status.is_terminal() && d.finished_at.is_none() {
            d.finished_at = Some(now);
        }
        if let Some(e) = error {
            d.error = Some(e.to_string());
        }
        self.update_deploy(&d).await?;
        Ok(d)
    }

    pub async fn get_deploy(&self, id: &str) -> Result<Option<Deploy>> {
        sqlx::query("SELECT * FROM deploys WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| row_to_deploy(&r))
            .transpose()
    }

    pub async fn require_deploy(&self, id: &str) -> Result<Deploy> {
        self.get_deploy(id).await?.ok_or_else(|| Error::not_found("deploy", id))
    }

    /// Newest first.
    pub async fn list_deploys(&self, service_id: &str, limit: u32) -> Result<Vec<Deploy>> {
        sqlx::query("SELECT * FROM deploys WHERE service_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?")
            .bind(service_id)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_deploy)
            .collect()
    }

    pub async fn latest_deploy(&self, service_id: &str) -> Result<Option<Deploy>> {
        Ok(self.list_deploys(service_id, 1).await?.into_iter().next())
    }

    /// Queued / building / deploying deploys of all services, oldest first.
    pub async fn active_deploys(&self) -> Result<Vec<Deploy>> {
        sqlx::query(
            "SELECT * FROM deploys WHERE status IN ('queued', 'building', 'deploying') ORDER BY created_at, rowid",
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(row_to_deploy)
        .collect()
    }

    /// Deploys of a service that recorded an image, newest first.
    pub async fn deploys_with_images(&self, service_id: &str) -> Result<Vec<Deploy>> {
        sqlx::query(
            "SELECT * FROM deploys WHERE service_id = ? AND image IS NOT NULL ORDER BY created_at DESC, rowid DESC",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(row_to_deploy)
        .collect()
    }

    // -----------------------------------------------------------------------
    // job runs

    pub async fn create_job_run(&self, j: &JobRun) -> Result<()> {
        sqlx::query(
            "INSERT INTO job_runs (id, service_id, trigger_kind, command, image, status, exit_code, error,
                created_at, started_at, finished_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&j.id)
        .bind(&j.service_id)
        .bind(j.trigger.as_str())
        .bind(&j.command)
        .bind(&j.image)
        .bind(j.status.as_str())
        .bind(j.exit_code)
        .bind(&j.error)
        .bind(ts(&j.created_at))
        .bind(ts_opt(&j.started_at))
        .bind(ts_opt(&j.finished_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_job_run(&self, j: &JobRun) -> Result<()> {
        let res = sqlx::query(
            "UPDATE job_runs SET command = ?, image = ?, status = ?, exit_code = ?, error = ?, started_at = ?,
                finished_at = ?
             WHERE id = ?",
        )
        .bind(&j.command)
        .bind(&j.image)
        .bind(j.status.as_str())
        .bind(j.exit_code)
        .bind(&j.error)
        .bind(ts_opt(&j.started_at))
        .bind(ts_opt(&j.finished_at))
        .bind(&j.id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("job", &j.id));
        }
        Ok(())
    }

    pub async fn get_job_run(&self, id: &str) -> Result<Option<JobRun>> {
        sqlx::query("SELECT * FROM job_runs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| row_to_job(&r))
            .transpose()
    }

    pub async fn require_job_run(&self, id: &str) -> Result<JobRun> {
        self.get_job_run(id).await?.ok_or_else(|| Error::not_found("job", id))
    }

    /// Newest first.
    pub async fn list_job_runs(&self, service_id: &str, limit: u32) -> Result<Vec<JobRun>> {
        sqlx::query("SELECT * FROM job_runs WHERE service_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?")
            .bind(service_id)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_job)
            .collect()
    }

    /// Pending / running jobs of all services, oldest first.
    pub async fn active_job_runs(&self) -> Result<Vec<JobRun>> {
        sqlx::query("SELECT * FROM job_runs WHERE status IN ('pending', 'running') ORDER BY created_at, rowid")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_job)
            .collect()
    }

    // -----------------------------------------------------------------------
    // datastores

    /// Insert a datastore. Conflict if the name is taken by a service or datastore.
    pub async fn create_datastore(&self, d: &Datastore) -> Result<()> {
        if self.name_taken(&d.name).await? {
            return Err(Error::conflict(format!("name '{}' is already in use", d.name)));
        }
        let res = sqlx::query(
            "INSERT INTO datastores (id, name, kind, version, status, username, password, database, host_port,
                error, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&d.id)
        .bind(&d.name)
        .bind(d.kind.as_str())
        .bind(&d.version)
        .bind(d.status.as_str())
        .bind(&d.username)
        .bind(&d.password)
        .bind(&d.database)
        .bind(d.host_port.map(i64::from))
        .bind(&d.error)
        .bind(ts(&d.created_at))
        .bind(ts(&d.updated_at))
        .execute(&self.pool)
        .await;
        match res {
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => Err(Error::conflict(format!("name '{}' is already in use", d.name))),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist every mutable column and bump `updated_at`. Returns the stored row.
    pub async fn update_datastore(&self, d: &Datastore) -> Result<Datastore> {
        let res = sqlx::query(
            "UPDATE datastores SET version = ?, status = ?, username = ?, password = ?, database = ?, host_port = ?,
                error = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&d.version)
        .bind(d.status.as_str())
        .bind(&d.username)
        .bind(&d.password)
        .bind(&d.database)
        .bind(d.host_port.map(i64::from))
        .bind(&d.error)
        .bind(ts(&Utc::now()))
        .bind(&d.id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("datastore", &d.id));
        }
        self.require_datastore(&d.id).await
    }

    pub async fn find_datastore(&self, id_or_name: &str) -> Result<Option<Datastore>> {
        sqlx::query("SELECT * FROM datastores WHERE id = ?1 OR name = ?1 LIMIT 1")
            .bind(id_or_name)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| row_to_datastore(&r))
            .transpose()
    }

    pub async fn require_datastore(&self, id_or_name: &str) -> Result<Datastore> {
        self.find_datastore(id_or_name).await?.ok_or_else(|| Error::not_found("datastore", id_or_name))
    }

    pub async fn list_datastores(&self) -> Result<Vec<Datastore>> {
        sqlx::query("SELECT * FROM datastores ORDER BY name")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_to_datastore)
            .collect()
    }

    pub async fn delete_datastore(&self, id: &str) -> Result<()> {
        let res = sqlx::query("DELETE FROM datastores WHERE id = ?").bind(id).execute(&self.pool).await?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("datastore", id));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // settings (small key/value store for server-level state)

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT value FROM settings WHERE key = ?").bind(key).fetch_optional(&self.pool).await?)
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a setting; returns whether it existed.
    pub async fn delete_setting(&self, key: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM settings WHERE key = ?").bind(key).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    // -----------------------------------------------------------------------
    // derived data

    /// Datastores and service references for [`crate::env::RefContext`].
    /// A service's port is its configured port, else its live deploy's port.
    pub async fn reference_targets(&self, config: &Config) -> Result<(Vec<Datastore>, Vec<ServiceRef>)> {
        let datastores = self.list_datastores().await?;
        let mut refs = Vec::new();
        for s in self.list_services().await? {
            let mut port = s.port;
            if port.is_none()
                && let Some(live) = &s.live_deploy_id
            {
                port = self.get_deploy(live).await?.and_then(|d| d.port);
            }
            refs.push(ServiceRef { name: s.name.clone(), port, public_url: config.service_url(&s) });
        }
        Ok((datastores, refs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn service_roundtrip_and_conflicts() {
        let store = Store::open_in_memory().await.unwrap();
        let mut svc = Service::new("web", ServiceType::WebService);
        svc.repo_url = Some("https://github.com/a/b".into());
        svc.port = Some(3000);
        svc.custom_domains = vec!["a.example.com".into()];
        store.create_service(&svc).await.unwrap();
        let got = store.require_service("web").await.unwrap();
        assert_eq!(got.id, svc.id);
        assert_eq!(got.custom_domains, svc.custom_domains);
        assert_eq!(got.port, Some(3000));
        assert_eq!(got.created_at, svc.created_at);

        let dup = Service::new("web", ServiceType::BackgroundWorker);
        assert!(matches!(store.create_service(&dup).await, Err(Error::Conflict(_))));
        let ds = Datastore::new("web", DatastoreKind::Redis);
        assert!(matches!(store.create_datastore(&ds).await, Err(Error::Conflict(_))));

        let mut upd = got.clone();
        upd.instances = 3;
        upd.suspended = true;
        let upd = store.update_service(&upd).await.unwrap();
        assert_eq!(upd.instances, 3);
        assert!(upd.suspended);
        store.set_instances(&svc.id, 2).await.unwrap();
        store.set_suspended(&svc.id, false).await.unwrap();
        let s = store.require_service(&svc.id).await.unwrap();
        assert_eq!((s.instances, s.suspended), (2, false));
        assert_eq!(store.list_services().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deploys_and_jobs() {
        let store = Store::open_in_memory().await.unwrap();
        let svc = Service::new("api", ServiceType::WebService);
        store.create_service(&svc).await.unwrap();
        let d1 = Deploy::new(&svc.id, DeployTrigger::Create, DeploySource::Image { image: "nginx".into() });
        store.create_deploy(&d1).await.unwrap();
        let mut d2 = Deploy::new(&svc.id, DeployTrigger::Manual, DeploySource::Image { image: "nginx".into() });
        d2.created_at = d1.created_at + chrono::Duration::milliseconds(5);
        store.create_deploy(&d2).await.unwrap();
        assert_eq!(store.latest_deploy(&svc.id).await.unwrap().unwrap().id, d2.id);
        assert_eq!(store.active_deploys().await.unwrap().len(), 2);

        let b = store.set_deploy_status(&d1.id, DeployStatus::Building, None).await.unwrap();
        assert!(b.started_at.is_some() && b.finished_at.is_none());
        let mut live = store.set_deploy_status(&d1.id, DeployStatus::Live, None).await.unwrap();
        assert!(live.finished_at.is_some());
        live.image = Some("nginx".into());
        live.port = Some(80);
        store.update_deploy(&live).await.unwrap();
        store.set_live_deploy(&svc.id, Some(&d1.id)).await.unwrap();
        let (_, refs) = store.reference_targets(&Config::default()).await.unwrap();
        assert_eq!(refs[0].port, Some(80));
        assert_eq!(store.deploys_with_images(&svc.id).await.unwrap().len(), 1);
        let f = store.set_deploy_status(&d2.id, DeployStatus::BuildFailed, Some("boom")).await.unwrap();
        assert_eq!(f.error.as_deref(), Some("boom"));
        assert!(store.active_deploys().await.unwrap().is_empty());

        let mut j = JobRun::new(&svc.id, JobTrigger::Manual, Some("echo hi".into()));
        store.create_job_run(&j).await.unwrap();
        assert_eq!(store.active_job_runs().await.unwrap().len(), 1);
        j.status = JobStatus::Succeeded;
        j.exit_code = Some(0);
        store.update_job_run(&j).await.unwrap();
        assert_eq!(store.require_job_run(&j.id).await.unwrap().exit_code, Some(0));
        assert_eq!(store.list_job_runs(&svc.id, 10).await.unwrap().len(), 1);

        store.delete_service(&svc.id).await.unwrap();
        assert!(store.get_deploy(&d1.id).await.unwrap().is_none());
        assert!(store.get_job_run(&j.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn env_and_groups() {
        let store = Store::open_in_memory().await.unwrap();
        let svc = Service::new("api", ServiceType::WebService);
        store.create_service(&svc).await.unwrap();
        let g1 = EnvGroup::new("shared");
        let g2 = EnvGroup::new("more");
        store.create_env_group(&g1).await.unwrap();
        store.create_env_group(&g2).await.unwrap();
        assert!(matches!(store.create_env_group(&EnvGroup::new("shared")).await, Err(Error::Conflict(_))));
        store.replace_env(&g1.id, &[EnvVar::new("A", "g1"), EnvVar::new("B", "g1")]).await.unwrap();
        store.replace_env(&g2.id, &[EnvVar::new("B", "g2")]).await.unwrap();
        store.link_env_group(&svc.id, &g1.id).await.unwrap();
        store.link_env_group(&svc.id, &g2.id).await.unwrap();
        store.link_env_group(&svc.id, &g1.id).await.unwrap(); // idempotent
        store.set_env(&svc.id, "C", "svc").await.unwrap();
        store.patch_env(&svc.id, &[EnvVar::new("A", "svc")], &["NOPE".into()]).await.unwrap();
        let env = store.effective_env(&svc.id).await.unwrap();
        assert_eq!(env, vec![EnvVar::new("A", "svc"), EnvVar::new("B", "g2"), EnvVar::new("C", "svc")]);
        let groups = store.service_env_groups(&svc.id).await.unwrap();
        assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["shared", "more"]);
        assert_eq!(store.env_group_services(&g1.id).await.unwrap().len(), 1);
        assert!(store.unlink_env_group(&svc.id, &g2.id).await.unwrap());
        store.delete_env_group(&g1.id).await.unwrap();
        assert_eq!(store.effective_env(&svc.id).await.unwrap(), vec![EnvVar::new("A", "svc"), EnvVar::new("C", "svc")]);
        assert!(store.unset_env(&svc.id, "C").await.unwrap());
        assert!(!store.unset_env(&svc.id, "C").await.unwrap());
    }

    #[tokio::test]
    async fn datastores_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("sub/ferry.db")).await.unwrap();
        let mut ds = Datastore::new("db", DatastoreKind::Postgres);
        store.create_datastore(&ds).await.unwrap();
        ds.status = DatastoreStatus::Available;
        ds.host_port = Some(54321);
        let ds2 = store.update_datastore(&ds).await.unwrap();
        assert_eq!(ds2.host_port, Some(54321));
        assert_eq!(store.require_datastore("db").await.unwrap().status, DatastoreStatus::Available);
        assert!(store.name_taken("db").await.unwrap());
        store.delete_datastore(&ds.id).await.unwrap();
        assert!(store.list_datastores().await.unwrap().is_empty());
        store.set_setting("k", "v1").await.unwrap();
        store.set_setting("k", "v2").await.unwrap();
        assert_eq!(store.get_setting("k").await.unwrap().as_deref(), Some("v2"));
        drop(store);
        // Reopen: migrations must be idempotent.
        let store = Store::open(&dir.path().join("sub/ferry.db")).await.unwrap();
        assert_eq!(store.get_setting("k").await.unwrap().as_deref(), Some("v2"));
    }
}
