//! Transactional migration of local control state into hosted PostgreSQL.
//!
//! DuckLake's local file catalog has no supported export/import API in this
//! checkout. Its presence aborts the migration before any target connection or
//! write, preventing a deceptively successful partial cutover.

use anyhow::{Context, Result};
use camino::Utf8Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    AdminImported,
}

pub fn migrate_admin_sqlite(source_dir: &Utf8Path, catalog_url: &str) -> Result<MigrationOutcome> {
    let sqlite_path = source_dir.join("admin.sqlite");
    let local_catalog = source_dir.join("default/catalog.ducklake");
    anyhow::ensure!(
        sqlite_path.exists(),
        "source admin database does not exist: {sqlite_path}"
    );
    anyhow::ensure!(
        !local_catalog.exists(),
        "refusing partial migration: local DuckLake metadata exists at {local_catalog}; this DuckLake build has no safe metadata export/import API"
    );

    let source = rusqlite::Connection::open(&sqlite_path)
        .with_context(|| format!("failed to open source admin database {sqlite_path}"))?;
    let mut target = crate::admin::connect_postgres(catalog_url)?;
    let mut tx = target
        .transaction()
        .context("failed to start hosted migration transaction")?;
    let result = (|| -> Result<()> {
        tx.batch_execute(
            "CREATE SCHEMA IF NOT EXISTS sigiro_admin;
             CREATE TABLE IF NOT EXISTS sigiro_admin.instance_status (
               key TEXT PRIMARY KEY, value TEXT NOT NULL,
               updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP);
             CREATE TABLE IF NOT EXISTS sigiro_admin.alert_instances (
               project_id TEXT NOT NULL, fingerprint TEXT NOT NULL, rule_id TEXT NOT NULL,
               status TEXT NOT NULL, service_name TEXT NOT NULL, container_name TEXT NOT NULL,
               starts_at_us BIGINT NOT NULL, ends_at_us BIGINT, occurrence_count BIGINT NOT NULL,
               updated_at_us BIGINT NOT NULL, PRIMARY KEY(project_id, fingerprint));
             CREATE TABLE IF NOT EXISTS sigiro_admin.alert_occurrences (
               project_id TEXT NOT NULL, occurrence_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
               observed_at_us BIGINT NOT NULL, PRIMARY KEY(project_id, occurrence_key));
             CREATE TABLE IF NOT EXISTS sigiro_admin.alert_outbox (
               id BIGSERIAL PRIMARY KEY, project_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
               transition TEXT NOT NULL, payload_json TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
               next_attempt_at_us BIGINT NOT NULL, sent_at_us BIGINT, last_error TEXT);",
        )?;

        copy_instance_status(&source, &mut tx)?;
        if sqlite_table_exists(&source, "alert_instances")? {
            copy_alert_instances(&source, &mut tx)?;
        }
        if sqlite_table_exists(&source, "alert_occurrences")? {
            copy_alert_occurrences(&source, &mut tx)?;
        }
        if sqlite_table_exists(&source, "alert_outbox")? {
            copy_alert_outbox(&source, &mut tx)?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            tx.commit().context("failed to commit hosted migration")?;
            Ok(MigrationOutcome::AdminImported)
        }
        Err(error) => {
            drop(tx.rollback());
            Err(error).context("hosted migration rolled back; source admin.sqlite was preserved")
        }
    }
}

fn sqlite_table_exists(source: &rusqlite::Connection, table: &str) -> Result<bool> {
    Ok(source.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?)
}

fn copy_instance_status(
    source: &rusqlite::Connection,
    tx: &mut postgres::Transaction<'_>,
) -> Result<()> {
    let mut statement = source.prepare("SELECT key, value FROM instance_status")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let value: String = row.get(1)?;
        tx.execute(
            "INSERT INTO sigiro_admin.instance_status(key,value) VALUES ($1,$2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP",
            &[&key, &value],
        )?;
    }
    Ok(())
}

fn copy_alert_instances(
    source: &rusqlite::Connection,
    tx: &mut postgres::Transaction<'_>,
) -> Result<()> {
    let mut statement = source.prepare(
        "SELECT project_id,fingerprint,rule_id,status,service_name,container_name,
                starts_at_us,ends_at_us,occurrence_count,updated_at_us FROM alert_instances",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let values = (
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, i64>(9)?,
        );
        tx.execute(
            "INSERT INTO sigiro_admin.alert_instances
             (project_id,fingerprint,rule_id,status,service_name,container_name,starts_at_us,ends_at_us,occurrence_count,updated_at_us)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
             ON CONFLICT(project_id,fingerprint) DO UPDATE SET
             rule_id=excluded.rule_id,status=excluded.status,service_name=excluded.service_name,
             container_name=excluded.container_name,starts_at_us=excluded.starts_at_us,
             ends_at_us=excluded.ends_at_us,occurrence_count=excluded.occurrence_count,
             updated_at_us=excluded.updated_at_us",
            &[&values.0,&values.1,&values.2,&values.3,&values.4,&values.5,&values.6,&values.7,&values.8,&values.9],
        )?;
    }
    Ok(())
}

fn copy_alert_occurrences(
    source: &rusqlite::Connection,
    tx: &mut postgres::Transaction<'_>,
) -> Result<()> {
    let mut statement = source.prepare(
        "SELECT project_id,occurrence_key,fingerprint,observed_at_us FROM alert_occurrences",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let project_id: String = row.get(0)?;
        let occurrence_key: String = row.get(1)?;
        let fingerprint: String = row.get(2)?;
        let observed_at_us: i64 = row.get(3)?;
        tx.execute(
            "INSERT INTO sigiro_admin.alert_occurrences(project_id,occurrence_key,fingerprint,observed_at_us)
             VALUES ($1,$2,$3,$4) ON CONFLICT(project_id,occurrence_key) DO UPDATE SET
             fingerprint=excluded.fingerprint, observed_at_us=excluded.observed_at_us",
            &[&project_id,&occurrence_key,&fingerprint,&observed_at_us],
        )?;
    }
    Ok(())
}

fn copy_alert_outbox(
    source: &rusqlite::Connection,
    tx: &mut postgres::Transaction<'_>,
) -> Result<()> {
    let mut statement = source.prepare(
        "SELECT id,project_id,fingerprint,transition,payload_json,attempts,
                next_attempt_at_us,sent_at_us,last_error FROM alert_outbox ORDER BY id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let values = (
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i32>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
        );
        tx.execute(
            "INSERT INTO sigiro_admin.alert_outbox
             (id,project_id,fingerprint,transition,payload_json,attempts,next_attempt_at_us,sent_at_us,last_error)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(id) DO UPDATE SET
             project_id=excluded.project_id, fingerprint=excluded.fingerprint,
             transition=excluded.transition, payload_json=excluded.payload_json,
             attempts=excluded.attempts, next_attempt_at_us=excluded.next_attempt_at_us,
             sent_at_us=excluded.sent_at_us, last_error=excluded.last_error",
            &[&values.0,&values.1,&values.2,&values.3,&values.4,&values.5,&values.6,&values.7,&values.8],
        )?;
    }
    tx.query_one(
        "SELECT setval(pg_get_serial_sequence('sigiro_admin.alert_outbox','id'),
          GREATEST(COALESCE((SELECT max(id) FROM sigiro_admin.alert_outbox), 1), 1),
          EXISTS(SELECT 1 FROM sigiro_admin.alert_outbox))",
        &[],
    )?;
    Ok(())
}

pub fn has_unsupported_ducklake_metadata(source_dir: &Utf8Path) -> bool {
    source_dir.join("default/catalog.ducklake").exists()
}
