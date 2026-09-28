use crate::{admin::AdminConnection, auth::AuthContext, server::AppState, types::TenantId};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    http::{HeaderMap, header},
};
use axum_anyhow::{ApiResult, bad_request, forbidden, internal_error};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectionInput {
    id: String,
    github_installation_id: i64,
    github_repository_id: i64,
    owner: String,
    repo: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingInput {
    service: String,
    connection_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectionStateInput {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TriggerInput {
    id: String,
    service: String,
    operation: String,
    signal: String,
    ts_shift_us: i64,
}

#[derive(Serialize)]
pub(crate) struct RunOutput {
    id: String,
    status: String,
    service: String,
    connection_id: String,
    finding_id: String,
    branch: String,
    actor: String,
    github_pull_request_id: Option<i64>,
    session_id: Option<String>,
    attempts: i32,
    failure_summary: Option<String>,
    target_ref: Option<String>,
    target_commit_sha: Option<String>,
    hypothesis: Option<String>,
    change_summary: Option<String>,
    validation_status: Option<String>,
    finding_before: Option<f64>,
    finding_after: Option<f64>,
    telemetry_window_start_us: Option<i64>,
    telemetry_window_end_us: Option<i64>,
}

fn valid_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && value != "."
        && value != ".."
}

fn db_error() -> axum_anyhow::ApiError {
    internal_error(
        "optimizer storage unavailable",
        "unable to access optimizer state",
    )
}

fn authorized(context: Option<Extension<AuthContext>>) -> ApiResult<AuthContext> {
    context.map(|Extension(value)| value).ok_or_else(|| {
        forbidden(
            "Hosted authorization required",
            "optimizer endpoints require hosted tenant authentication",
        )
    })
}

pub(crate) fn create_local(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS optimizer_connection (
        tenant_id TEXT NOT NULL, id TEXT NOT NULL, installation_id INTEGER NOT NULL,
        repository_id INTEGER NOT NULL, owner TEXT NOT NULL, repo TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1, PRIMARY KEY (tenant_id, id));
        CREATE TABLE IF NOT EXISTS optimizer_mapping (
        tenant_id TEXT NOT NULL, service TEXT NOT NULL, connection_id TEXT NOT NULL,
        PRIMARY KEY (tenant_id, service, connection_id));
        CREATE TABLE IF NOT EXISTS optimizer_run (
        tenant_id TEXT NOT NULL, id TEXT NOT NULL, service TEXT NOT NULL,
        connection_id TEXT NOT NULL, finding_id TEXT NOT NULL, branch TEXT NOT NULL,
        actor TEXT NOT NULL, status TEXT NOT NULL,
        lease_owner TEXT, lease_until_ms INTEGER, attempts INTEGER NOT NULL DEFAULT 0,
        session_id TEXT, failure_summary TEXT, github_pull_request_id INTEGER,
        target_ref TEXT, target_commit_sha TEXT, hypothesis TEXT, change_summary TEXT, validation_status TEXT,
        finding_before DOUBLE PRECISION, finding_after DOUBLE PRECISION,
        telemetry_window_start_us BIGINT, telemetry_window_end_us BIGINT,
        PRIMARY KEY (tenant_id, id));",
    )?;
    let fields = [
        ("lease_owner", "TEXT"),
        ("lease_until_ms", "INTEGER"),
        ("attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("session_id", "TEXT"),
        ("failure_summary", "TEXT"),
        ("github_pull_request_id", "INTEGER"),
        ("target_ref", "TEXT"),
        ("target_commit_sha", "TEXT"),
        ("hypothesis", "TEXT"),
        ("change_summary", "TEXT"),
        ("validation_status", "TEXT"),
        ("finding_before", "REAL"),
        ("finding_after", "REAL"),
        ("telemetry_window_start_us", "INTEGER"),
        ("telemetry_window_end_us", "INTEGER"),
    ];
    let mut stmt = conn.prepare("PRAGMA table_info(optimizer_run)")?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    for (name, ty) in fields {
        if !existing.contains(name) {
            conn.execute_batch(&format!("ALTER TABLE optimizer_run ADD COLUMN {name} {ty}"))?;
        }
    }
    Ok(())
}

pub(crate) fn create_hosted(conn: &mut postgres::Client) -> anyhow::Result<()> {
    conn.batch_execute(
        "CREATE TABLE IF NOT EXISTS sigiro_admin.optimizer_connection (
        tenant_id TEXT NOT NULL, id TEXT NOT NULL, installation_id BIGINT NOT NULL,
        repository_id BIGINT NOT NULL, owner TEXT NOT NULL, repo TEXT NOT NULL,
        enabled BOOLEAN NOT NULL DEFAULT TRUE, PRIMARY KEY (tenant_id, id));
        CREATE TABLE IF NOT EXISTS sigiro_admin.optimizer_mapping (
        tenant_id TEXT NOT NULL, service TEXT NOT NULL, connection_id TEXT NOT NULL,
        PRIMARY KEY (tenant_id, service, connection_id));
        CREATE TABLE IF NOT EXISTS sigiro_admin.optimizer_run (
        tenant_id TEXT NOT NULL, id TEXT NOT NULL, service TEXT NOT NULL,
        connection_id TEXT NOT NULL, finding_id TEXT NOT NULL, branch TEXT NOT NULL,
        actor TEXT NOT NULL, status TEXT NOT NULL,
        lease_owner TEXT, lease_until_ms BIGINT, attempts INTEGER NOT NULL DEFAULT 0,
        session_id TEXT, failure_summary TEXT, github_pull_request_id BIGINT,
        target_ref TEXT, target_commit_sha TEXT, hypothesis TEXT, change_summary TEXT, validation_status TEXT,
        finding_before DOUBLE PRECISION, finding_after DOUBLE PRECISION,
        telemetry_window_start_us BIGINT, telemetry_window_end_us BIGINT,
        PRIMARY KEY (tenant_id, id));
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS lease_owner TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS lease_until_ms BIGINT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS attempts INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS session_id TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS failure_summary TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS github_pull_request_id BIGINT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS target_ref TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS target_commit_sha TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS hypothesis TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS change_summary TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS validation_status TEXT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS finding_before DOUBLE PRECISION;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS finding_after DOUBLE PRECISION;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS telemetry_window_start_us BIGINT;
        ALTER TABLE sigiro_admin.optimizer_run ADD COLUMN IF NOT EXISTS telemetry_window_end_us BIGINT;",
    )?;
    Ok(())
}

fn read_run(
    conn: &mut AdminConnection,
    tenant: &str,
    id: &str,
) -> anyhow::Result<Option<RunOutput>> {
    let columns = "service,connection_id,finding_id,branch,actor,status,github_pull_request_id,session_id,attempts,failure_summary,target_ref,target_commit_sha,hypothesis,change_summary,validation_status,finding_before,finding_after,telemetry_window_start_us,telemetry_window_end_us";
    let record = match conn {
        AdminConnection::Local(conn) => {
            use rusqlite::OptionalExtension;
            conn.query_row(
                &format!("SELECT {columns} FROM optimizer_run WHERE tenant_id=?1 AND id=?2"),
                rusqlite::params![tenant, id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i32>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, Option<String>>(14)?,
                        row.get::<_, Option<f64>>(15)?,
                        row.get::<_, Option<f64>>(16)?,
                        row.get::<_, Option<i64>>(17)?,
                        row.get::<_, Option<i64>>(18)?,
                    ))
                },
            )
            .optional()?
        }
        AdminConnection::Hosted(conn) => conn
            .query_opt(
                &format!(
                    "SELECT {columns} FROM sigiro_admin.optimizer_run WHERE tenant_id=$1 AND id=$2"
                ),
                &[&tenant, &id],
            )?
            .map(|row| {
                (
                    row.get(0),
                    row.get(1),
                    row.get(2),
                    row.get(3),
                    row.get(4),
                    row.get(5),
                    row.get(6),
                    row.get(7),
                    row.get(8),
                    row.get(9),
                    row.get(10),
                    row.get(11),
                    row.get(12),
                    row.get(13),
                    row.get(14),
                    row.get(15),
                    row.get(16),
                    row.get(17),
                    row.get(18),
                )
            }),
    };
    Ok(record.map(
        |(
            service,
            connection_id,
            finding_id,
            branch,
            actor,
            status,
            github_pull_request_id,
            session_id,
            attempts,
            failure_summary,
            target_ref,
            target_commit_sha,
            hypothesis,
            change_summary,
            validation_status,
            finding_before,
            finding_after,
            telemetry_window_start_us,
            telemetry_window_end_us,
        )| RunOutput {
            id: id.to_owned(),
            service,
            connection_id,
            finding_id,
            branch,
            actor,
            status,
            github_pull_request_id,
            session_id,
            attempts,
            failure_summary,
            target_ref,
            target_commit_sha,
            hypothesis,
            change_summary,
            validation_status,
            finding_before,
            finding_after,
            telemetry_window_start_us,
            telemetry_window_end_us,
        },
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InternalQueryInput {
    run_id: String,
    lease_owner: String,
    attempts: i32,
    sql: String,
}

pub(crate) async fn internal_query(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<InternalQueryInput>,
) -> ApiResult<Json<serde_json::Value>> {
    let expected = crate::config::OPTIMIZER_INTERNAL_TOKEN
        .value()
        .map_err(|_| db_error())?;
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if expected.len() < 32
        || supplied.len() != expected.len()
        || supplied
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
            != 0
    {
        return Err(axum_anyhow::unauthorized(
            "Internal authorization required",
            "invalid worker credential",
        ));
    }
    if !valid_id(&input.run_id)
        || !valid_id(&input.lease_owner)
        || input.attempts < 1
        || input.sql.len() > 4096
        || input.sql.is_empty()
    {
        return Err(bad_request(
            "Invalid optimizer query",
            "invalid run, lease, or SQL length",
        ));
    }
    crate::sql::validate_read_only_with(&input.sql, false)
        .map_err(|_| bad_request("Query rejected", "SQL must be a permitted read-only query"))?;
    let control = Arc::clone(&state);
    let run_id = input.run_id.clone();
    let lease_owner = input.lease_owner.clone();
    let attempts = input.attempts;
    let tenant = tokio::task::spawn_blocking(move || {
        let mut guard = control.admin_conn.lock();
        let row: Option<String> = match &mut *guard {
            AdminConnection::Local(conn) => {
                use rusqlite::OptionalExtension;
                conn.query_row("SELECT r.tenant_id FROM optimizer_run r JOIN optimizer_connection c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.id=?1 AND r.lease_owner=?2 AND r.attempts=?3 AND r.status='running' AND r.lease_until_ms >= (strftime('%s','now')*1000) AND c.enabled=1", rusqlite::params![run_id,lease_owner,attempts], |row| row.get(0)).optional()?
            },
            AdminConnection::Hosted(conn) => conn.query_opt("SELECT r.tenant_id FROM sigiro_admin.optimizer_run r JOIN sigiro_admin.optimizer_connection c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id WHERE r.id=$1 AND r.lease_owner=$2 AND r.attempts=$3 AND r.status='running' AND r.lease_until_ms >= (extract(epoch FROM clock_timestamp())*1000)::bigint AND c.enabled=TRUE", &[&run_id,&lease_owner,&attempts])?.map(|row| row.get(0)),
        };
        Ok::<_,anyhow::Error>(row)
    }).await.map_err(|_| db_error())?.map_err(|_| db_error())?;
    let tenant = tenant.ok_or_else(|| {
        axum_anyhow::forbidden(
            "Lease not active",
            "optimizer run is not authorized for tenant telemetry",
        )
    })?;
    let tenant = TenantId::try_new(tenant).map_err(|_| db_error())?;
    let (rows, truncated) = crate::server::execute_sql(&state, &tenant, &input.sql)
        .await
        .map_err(|_| db_error())?;
    let payload = serde_json::json!({"rows":rows,"truncated":truncated});
    if serde_json::to_vec(&payload).map_err(|_| db_error())?.len() > 100_000 {
        return Err(bad_request(
            "Optimizer result too large",
            "reduce the query window or projection",
        ));
    }
    Ok(Json(payload))
}

pub(crate) async fn connection(
    State(state): State<Arc<AppState>>,
    context: Option<Extension<AuthContext>>,
    Json(input): Json<ConnectionInput>,
) -> ApiResult<StatusCode> {
    let context = authorized(context)?;
    if !valid_id(&input.id)
        || input.github_installation_id <= 0
        || input.github_repository_id <= 0
        || !valid_slug(&input.owner)
        || !valid_slug(&input.repo)
    {
        return Err(bad_request(
            "Invalid connection",
            "expected UUID, positive GitHub identifiers, and repository slugs",
        ));
    }
    let tenant = context.tenant_id.to_string();
    let consistent = tokio::task::spawn_blocking(move || {
        let mut guard = state.admin_conn.lock();
        let saved: (i64, i64, String, String) = match &mut *guard {
            AdminConnection::Local(conn) => {
                conn.execute("INSERT OR IGNORE INTO optimizer_connection (tenant_id,id,installation_id,repository_id,owner,repo) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![tenant,input.id,input.github_installation_id,input.github_repository_id,input.owner,input.repo])?;
                conn.query_row("SELECT installation_id,repository_id,owner,repo FROM optimizer_connection WHERE tenant_id=?1 AND id=?2", rusqlite::params![tenant,input.id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?
            }
            AdminConnection::Hosted(conn) => {
                conn.execute("INSERT INTO sigiro_admin.optimizer_connection (tenant_id,id,installation_id,repository_id,owner,repo) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING", &[&tenant,&input.id,&input.github_installation_id,&input.github_repository_id,&input.owner,&input.repo])?;
                let row = conn.query_one("SELECT installation_id,repository_id,owner,repo FROM sigiro_admin.optimizer_connection WHERE tenant_id=$1 AND id=$2", &[&tenant,&input.id])?;
                (row.get(0), row.get(1), row.get(2), row.get(3))
            }
        };
        Ok::<_, anyhow::Error>(saved == (input.github_installation_id, input.github_repository_id, input.owner, input.repo))
    }).await.map_err(|_| db_error())?.map_err(|_| db_error())?;
    if !consistent {
        return Err(bad_request(
            "Connection identifier conflict",
            "this ID belongs to a different repository",
        ));
    }
    Ok(StatusCode::CREATED)
}

pub(crate) async fn set_connection(
    State(state): State<Arc<AppState>>,
    context: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
    Json(input): Json<ConnectionStateInput>,
) -> ApiResult<StatusCode> {
    let context = authorized(context)?;
    if !valid_id(&id) {
        return Err(bad_request("Invalid connection", "expected UUID"));
    }
    let tenant = context.tenant_id.to_string();
    let updated = tokio::task::spawn_blocking(move || {
        let mut guard = state.admin_conn.lock();
        let count = match &mut *guard {
            AdminConnection::Local(conn) => conn.execute("UPDATE optimizer_connection SET enabled=?3 WHERE tenant_id=?1 AND id=?2", rusqlite::params![tenant,id,input.enabled])? as u64,
            AdminConnection::Hosted(conn) => conn.execute("UPDATE sigiro_admin.optimizer_connection SET enabled=$3 WHERE tenant_id=$1 AND id=$2", &[&tenant,&id,&input.enabled])?,
        };
        Ok::<_,anyhow::Error>(count)
    }).await.map_err(|_| db_error())?.map_err(|_| db_error())?;
    if updated != 1 {
        return Err(bad_request(
            "Unknown connection",
            "connection does not belong to this tenant",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn mapping(
    State(state): State<Arc<AppState>>,
    context: Option<Extension<AuthContext>>,
    Json(input): Json<MappingInput>,
) -> ApiResult<StatusCode> {
    let context = authorized(context)?;
    if input.service.is_empty() || input.service.len() > 256 || !valid_id(&input.connection_id) {
        return Err(bad_request(
            "Invalid mapping",
            "service or connection identifier is invalid",
        ));
    }
    let tenant = context.tenant_id.to_string();
    let found = tokio::task::spawn_blocking(move || {
        let mut guard = state.admin_conn.lock();
        let found = match &mut *guard {
            AdminConnection::Local(conn) => {
                let found: i64 = conn.query_row("SELECT count(*) FROM optimizer_connection WHERE tenant_id=?1 AND id=?2 AND enabled=1", rusqlite::params![tenant,input.connection_id], |row| row.get(0))?;
                if found == 1 { conn.execute("INSERT OR IGNORE INTO optimizer_mapping (tenant_id,service,connection_id) VALUES (?1,?2,?3)", rusqlite::params![tenant,input.service,input.connection_id])?; }
                found
            },
            AdminConnection::Hosted(conn) => {
                let found: i64 = conn.query_one("SELECT count(*) FROM sigiro_admin.optimizer_connection WHERE tenant_id=$1 AND id=$2 AND enabled=TRUE", &[&tenant,&input.connection_id])?.get(0);
                if found == 1 { conn.execute("INSERT INTO sigiro_admin.optimizer_mapping (tenant_id,service,connection_id) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING", &[&tenant,&input.service,&input.connection_id])?; }
                found
            },
        };
        Ok::<_, anyhow::Error>(found)
    }).await.map_err(|_| db_error())?.map_err(|_| db_error())?;
    if found != 1 {
        return Err(bad_request(
            "Unknown connection",
            "repository connection is not enabled for this tenant",
        ));
    }
    Ok(StatusCode::CREATED)
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

pub(crate) async fn trigger(
    State(state): State<Arc<AppState>>,
    context: Option<Extension<AuthContext>>,
    Json(input): Json<TriggerInput>,
) -> ApiResult<Json<RunOutput>> {
    let context = authorized(context)?;
    if !valid_id(&input.id)
        || input.service.is_empty()
        || input.service.len() > 256
        || input.operation.len() > 256
        || input.signal.len() > 64
        || !(1_000_000_000_000_000..10_000_000_000_000_000).contains(&input.ts_shift_us)
    {
        return Err(bad_request(
            "Invalid finding",
            "invalid id, service, operation, signal or shift time",
        ));
    }
    let tenant = context.tenant_id.clone();
    let actor = context.subject;
    let finding_id = serde_json::to_string(&(
        &input.service,
        &input.operation,
        &input.signal,
        input.ts_shift_us,
    ))
    .expect("strings and integers are JSON serializable");
    let existing_state = Arc::clone(&state);
    let existing_tenant = tenant.to_string();
    let existing_id = input.id.clone();
    let previous = tokio::task::spawn_blocking(move || {
        let mut guard = existing_state.admin_conn.lock();
        read_run(&mut guard, &existing_tenant, &existing_id)
    })
    .await
    .map_err(|_| db_error())?
    .map_err(|_| db_error())?;
    if let Some(run) = previous {
        if run.finding_id != finding_id || run.actor != actor {
            return Err(bad_request(
                "Run identifier conflict",
                "this run ID belongs to a different request",
            ));
        }
        return Ok(Json(run));
    }
    let sql = format!(
        "SELECT \"before\", \"after\" FROM {}sigiro_anomalies WHERE service_name='{}' AND operation='{}' AND signal='{}' AND epoch_us(ts_shift)={} LIMIT 2",
        crate::pool::ProjectPool::prefix(),
        escape_sql(&input.service),
        escape_sql(&input.operation),
        escape_sql(&input.signal),
        input.ts_shift_us
    );
    let (rows, _) = crate::server::execute_sql(&state, &tenant, &sql)
        .await
        .map_err(|_| db_error())?;
    if rows.len() != 1 {
        return Err(bad_request(
            "Finding unavailable",
            "the requested finding is not uniquely present in this tenant's telemetry",
        ));
    }
    let finding_before = rows[0]
        .get("before")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| bad_request("Finding unavailable", "invalid original telemetry baseline"))?;
    let finding_after = rows[0]
        .get("after")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| bad_request("Finding unavailable", "invalid original telemetry baseline"))?;
    let telemetry_window_start_us = input.ts_shift_us - 900_000_000;
    let telemetry_window_end_us = input.ts_shift_us + 900_000_000;
    let branch = format!("sigiro/optimizer-{}", input.id);
    let tenant = tenant.to_string();
    let expected_finding_id = finding_id.clone();
    let expected_actor = actor.clone();
    let run = tokio::task::spawn_blocking(move || {
        let mut guard = state.admin_conn.lock();
        let connections = match &mut *guard {
            AdminConnection::Local(conn) => {
                let mut stmt = conn.prepare("SELECT c.id FROM optimizer_mapping m JOIN optimizer_connection c ON c.tenant_id=m.tenant_id AND c.id=m.connection_id WHERE m.tenant_id=?1 AND m.service=?2 AND c.enabled=1 LIMIT 2")?;
                let values = stmt.query_map(rusqlite::params![tenant,input.service], |row| row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                values
            },
            AdminConnection::Hosted(conn) => conn.query("SELECT c.id FROM sigiro_admin.optimizer_mapping m JOIN sigiro_admin.optimizer_connection c ON c.tenant_id=m.tenant_id AND c.id=m.connection_id WHERE m.tenant_id=$1 AND m.service=$2 AND c.enabled=TRUE LIMIT 2", &[&tenant,&input.service])?.into_iter().map(|row| row.get::<_,String>(0)).collect(),
        };
        if connections.len() != 1 { return Ok::<_,anyhow::Error>(None); }
        let connection_id = connections[0].clone();
        match &mut *guard {
            AdminConnection::Local(conn) => { conn.execute("INSERT OR IGNORE INTO optimizer_run (tenant_id,id,service,connection_id,finding_id,branch,actor,status,finding_before,finding_after,telemetry_window_start_us,telemetry_window_end_us) VALUES (?1,?2,?3,?4,?5,?6,?7,'pending',?8,?9,?10,?11)", rusqlite::params![tenant,input.id,input.service,connection_id,finding_id,branch,actor,finding_before,finding_after,telemetry_window_start_us,telemetry_window_end_us])?; },
            AdminConnection::Hosted(conn) => { conn.execute("INSERT INTO sigiro_admin.optimizer_run (tenant_id,id,service,connection_id,finding_id,branch,actor,status,finding_before,finding_after,telemetry_window_start_us,telemetry_window_end_us) VALUES ($1,$2,$3,$4,$5,$6,$7,'pending',$8,$9,$10,$11) ON CONFLICT DO NOTHING", &[&tenant,&input.id,&input.service,&connection_id,&finding_id,&branch,&actor,&finding_before,&finding_after,&telemetry_window_start_us,&telemetry_window_end_us])?; },
        }
        read_run(&mut guard, &tenant, &input.id)
    }).await.map_err(|_| db_error())?.map_err(|_| db_error())?;
    let Some(run) = run else {
        return Err(bad_request(
            "Repository mapping missing or ambiguous",
            "map the finding's service to exactly one enabled connection",
        ));
    };
    if run.finding_id != expected_finding_id || run.actor != expected_actor {
        return Err(bad_request(
            "Run identifier conflict",
            "this run ID belongs to a different request",
        ));
    }
    Ok(Json(run))
}

pub(crate) async fn get_run(
    State(state): State<Arc<AppState>>,
    context: Option<Extension<AuthContext>>,
    Path(id): Path<String>,
) -> ApiResult<Json<RunOutput>> {
    let tenant: TenantId = authorized(context)?.tenant_id;
    if !valid_id(&id) {
        return Err(bad_request("Invalid run ID", "expected UUID"));
    }
    let row = tokio::task::spawn_blocking(move || {
        let mut guard = state.admin_conn.lock();
        let tenant = tenant.to_string();
        read_run(&mut guard, &tenant, &id)
    })
    .await
    .map_err(|_| db_error())?
    .map_err(|_| db_error())?;
    row.map(Json)
        .ok_or_else(|| bad_request("Run not found", "run ID does not belong to this tenant"))
}
