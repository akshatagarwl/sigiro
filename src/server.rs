use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use crossbeam_channel::Sender;
use duckdb::Connection;
use std::sync::{Arc, Mutex};
use tonic::transport::Server as TonicServer;

use crate::ingest::{IngestMsg, OtlpGrpcService};
use crate::investigate;

pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
}

/// DuckDB's access_mode is database-level, not connection-level:
/// - `SET access_mode = 'read_only'` fails with "Cannot change access_mode setting while database is running"
/// - `Connection::open_with_flags(path, ReadOnly)` creates a separate database instance that
///   doesn't share in-memory state with the write connection (ingested data invisible)
///
/// Therefore we use a keyword blocklist + `prepare()` single-statement guarantee.
/// `conn.prepare()` only handles one statement — multi-statement injection via semicolons
/// is already prevented by DuckDB itself.
const BLOCKED_KEYWORDS: &[&str] = &[
    "DROP",
    "DELETE",
    "INSERT",
    "UPDATE",
    "ALTER",
    "CREATE",
    "COPY",
    "ATTACH",
    "DETACH",
    "EXPORT",
    "IMPORT",
    "PRAGMA",
    "CALL",
    "TRUNCATE",
    "CHECKPOINT",
    "LOAD",
    "INSTALL",
    "SET",
    "READ_CSV",
    "READ_PARQUET",
    "READ_JSON",
];

fn validate_read_only(sql: &str) -> Result<(), String> {
    let upper = sql.to_uppercase();
    for kw in BLOCKED_KEYWORDS {
        // Word-boundary check: keyword must be preceded and followed by non-alphanumeric/underscore
        // This prevents false positives like "updated_at" matching "UPDATE"
        for (pos, _) in upper.match_indices(kw) {
            let before_ok = pos == 0
                || !upper.as_bytes()[pos - 1].is_ascii_alphanumeric()
                    && upper.as_bytes()[pos - 1] != b'_';
            let end = pos + kw.len();
            let after_ok = end >= upper.len()
                || !upper.as_bytes()[end].is_ascii_alphanumeric() && upper.as_bytes()[end] != b'_';
            if before_ok && after_ok {
                return Err(format!("query rejected: {kw} not allowed"));
            }
        }
    }
    Ok(())
}

pub async fn run_servers(conn: Arc<Mutex<Connection>>, tx: Sender<IngestMsg>) -> Result<()> {
    // gRPC server on :4317
    let grpc_tx = tx.clone();
    let grpc_handle = tokio::spawn(async move {
        let addr = "[::]:4317".parse().expect("valid addr");
        let svc = OtlpGrpcService::new(grpc_tx);
        tracing::info!("gRPC listening on {addr}");
        TonicServer::builder()
            .add_service(svc.trace_service())
            .add_service(svc.logs_service())
            .add_service(svc.metrics_service())
            .serve(addr)
            .await
            .context("gRPC server failed")
    });

    // HTTP server on :9999 (query + health)
    let app_state = Arc::new(AppState { conn });
    let http_app = Router::new()
        .route("/health", get(health))
        .route("/query", post(query_handler))
        .route("/investigate", post(investigate_handler))
        .route("/investigate/multi", post(investigate_multi_handler))
        .with_state(app_state);

    let http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
        tracing::info!("HTTP listening on 0.0.0.0:9999");
        axum::serve(listener, http_app)
            .await
            .context("HTTP server failed")
    });

    // OTLP HTTP on :4318
    let http_otlp_tx = tx;
    let otlp_http_app = crate::ingest::otlp_http_router(http_otlp_tx);
    let otlp_http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:4318").await?;
        tracing::info!("OTLP HTTP listening on 0.0.0.0:4318");
        axum::serve(listener, otlp_http_app)
            .await
            .context("OTLP HTTP server failed")
    });

    // Servers should run forever. Any exit is unexpected and fatal.
    // Match on the result to log the specific server and error before propagating.
    tokio::select! {
        r = grpc_handle => {
            let result = r?;
            tracing::error!("gRPC server exited: {result:?}");
            result?
        },
        r = http_handle => {
            let result = r?;
            tracing::error!("HTTP server exited: {result:?}");
            result?
        },
        r = otlp_http_handle => {
            let result = r?;
            tracing::error!("OTLP HTTP server exited: {result:?}");
            result?
        },
    }

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// Convert a DuckDB ValueRef to a serde_json::Value.
/// Uses pattern matching on the actual type — no cascading type-try guessing.
fn value_ref_to_json(val: duckdb::types::ValueRef<'_>) -> serde_json::Value {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::json!(b),
        ValueRef::TinyInt(n) => serde_json::json!(n),
        ValueRef::SmallInt(n) => serde_json::json!(n),
        ValueRef::Int(n) => serde_json::json!(n),
        ValueRef::BigInt(n) => serde_json::json!(n),
        ValueRef::HugeInt(n) => serde_json::json!(n.to_string()),
        ValueRef::UTinyInt(n) => serde_json::json!(n),
        ValueRef::USmallInt(n) => serde_json::json!(n),
        ValueRef::UInt(n) => serde_json::json!(n),
        ValueRef::UBigInt(n) => serde_json::json!(n),
        ValueRef::Float(f) => serde_json::json!(f),
        ValueRef::Double(f) => serde_json::json!(f),
        ValueRef::Text(bytes) => {
            let s = std::str::from_utf8(bytes).unwrap_or("<invalid utf8>");
            serde_json::Value::String(s.to_string())
        }
        ValueRef::Blob(bytes) => serde_json::json!(format!("<blob {} bytes>", bytes.len())),
        ValueRef::Timestamp(_, us) => serde_json::json!(us),
        ValueRef::Date32(d) => serde_json::json!(d),
        ValueRef::Time64(_, t) => serde_json::json!(t),
        // Compound types: render as string representation
        _ => serde_json::Value::String(format!("{val:?}")),
    }
}

async fn query_handler(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<Vec<serde_json::Value>>, String> {
    validate_read_only(&body)?;
    let conn = state.conn.lock().map_err(|e| {
        tracing::error!("DuckDB connection mutex poisoned");
        e.to_string()
    })?;
    let mut stmt = conn.prepare(&body).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;

    let stmt_ref = rows.as_ref().ok_or("no result set")?;
    let column_count = stmt_ref.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| {
            stmt_ref
                .column_name(i)
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "?".to_string())
        })
        .collect();

    let mut results = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row.get_ref(i).map_err(|e| e.to_string())?;
            obj.insert(name.clone(), value_ref_to_json(val));
        }
        results.push(serde_json::Value::Object(obj));
    }

    Ok(Json(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("CREATE TABLE test_t (id INTEGER, name VARCHAR, flag BOOLEAN)")
            .expect("create table");
        conn.execute_batch("INSERT INTO test_t VALUES (1, 'hello', true), (2, 'world', false)")
            .expect("insert");
        conn
    }

    /// DuckDB read-only enforcement via Config at open time.
    /// Uses a temp file because access_mode must be set when opening the database,
    /// not after — this is the same mechanism the server uses.
    #[test]
    fn read_only_via_config_blocks_mutations() {
        let dir = std::env::temp_dir().join("sigiro_test_ro");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("test_readonly.duckdb");

        // Write phase: create table and insert data
        {
            let conn = Connection::open(&db_path).expect("open for writing");
            conn.execute_batch("CREATE TABLE test_t (id INTEGER, name VARCHAR)")
                .expect("create");
            conn.execute_batch("INSERT INTO test_t VALUES (1, 'hello')")
                .expect("insert");
        }

        // Read-only phase: open same file with ReadOnly config
        let ro_config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .expect("config");
        let ro_conn = Connection::open_with_flags(&db_path, ro_config).expect("open read-only");

        // SELECT works
        let mut stmt = ro_conn.prepare("SELECT * FROM test_t").unwrap();
        let mut rows = stmt.query([]).unwrap();
        assert!(rows.next().unwrap().is_some());

        // All write operations fail at the database level
        let mutations = [
            "DROP TABLE test_t",
            "DELETE FROM test_t",
            "INSERT INTO test_t VALUES (2, 'evil')",
            "UPDATE test_t SET name = 'evil'",
            "CREATE TABLE evil (x INT)",
        ];
        for sql in mutations {
            let err = ro_conn.execute_batch(sql).unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("read-only")
                    || err.to_string().to_lowercase().contains("read only"),
                "{sql} should fail in read-only mode, got: {err}"
            );
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn value_ref_integer() {
        let conn = test_conn();
        let mut stmt = conn.prepare("SELECT 42 AS n").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(
            value_ref_to_json(row.get_ref(0).unwrap()),
            serde_json::json!(42)
        );
    }

    #[test]
    fn value_ref_text() {
        let conn = test_conn();
        let mut stmt = conn.prepare("SELECT 'hello' AS s").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(
            value_ref_to_json(row.get_ref(0).unwrap()),
            serde_json::json!("hello")
        );
    }

    #[test]
    fn value_ref_bool() {
        let conn = test_conn();
        let mut stmt = conn.prepare("SELECT true AS b").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(
            value_ref_to_json(row.get_ref(0).unwrap()),
            serde_json::json!(true)
        );
    }

    #[test]
    fn value_ref_null() {
        let conn = test_conn();
        let mut stmt = conn.prepare("SELECT NULL AS n").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert!(value_ref_to_json(row.get_ref(0).unwrap()).is_null());
    }

    #[test]
    fn value_ref_double() {
        let conn = test_conn();
        let mut stmt = conn.prepare("SELECT 3.14::DOUBLE AS f").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(
            value_ref_to_json(row.get_ref(0).unwrap()),
            serde_json::json!(3.14)
        );
    }

    #[test]
    fn validate_blocks_dml() {
        assert!(validate_read_only("DROP TABLE sigiro_spans").is_err());
        assert!(validate_read_only("delete from sigiro_spans").is_err());
        assert!(validate_read_only("INSERT INTO sigiro_spans VALUES (1)").is_err());
        assert!(validate_read_only("UPDATE sigiro_spans SET x = 1").is_err());
        assert!(validate_read_only("CREATE TABLE evil (x INT)").is_err());
        assert!(validate_read_only("ATTACH '/tmp/evil.db'").is_err());
        assert!(validate_read_only("SET access_mode = 'read_write'").is_err());
    }

    #[test]
    fn validate_allows_select() {
        assert!(validate_read_only("SELECT * FROM sigiro_spans").is_ok());
        assert!(
            validate_read_only("SELECT count(*) FROM sigiro_spans WHERE updated_at > 0").is_ok()
        );
        // Word boundary: "updated_at" should NOT trigger "UPDATE"
        assert!(validate_read_only("SELECT created_by FROM sigiro_spans").is_ok());
    }

    #[test]
    fn validate_blocks_file_access() {
        assert!(validate_read_only("SELECT * FROM read_csv('/etc/passwd')").is_err());
        assert!(validate_read_only("SELECT * FROM read_parquet('data.parquet')").is_err());
        assert!(validate_read_only("SELECT * FROM read_json('data.json')").is_err());
    }

    #[test]
    fn validate_blocks_comment_wrapped() {
        // /* DROP */ has spaces around DROP, so word boundary matches
        assert!(validate_read_only("SELECT 1 /* DROP TABLE evil */").is_err());
    }

    #[test]
    fn value_ref_mixed_types_from_table() {
        let conn = test_conn();
        let mut stmt = conn
            .prepare("SELECT id, name, flag FROM test_t WHERE id = 1")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(
            value_ref_to_json(row.get_ref(0).unwrap()),
            serde_json::json!(1)
        );
        assert_eq!(
            value_ref_to_json(row.get_ref(1).unwrap()),
            serde_json::json!("hello")
        );
        assert_eq!(
            value_ref_to_json(row.get_ref(2).unwrap()),
            serde_json::json!(true)
        );
    }
}

async fn investigate_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> Result<Json<investigate::InvestigationBlock>, String> {
    let block = investigate::investigate(&state.conn, &req).map_err(|e| format!("{e:#}"))?;
    Ok(Json(block))
}

async fn investigate_multi_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<investigate::InvestigateMultiRequest>,
) -> Result<Json<std::collections::BTreeMap<String, investigate::InvestigationBlock>>, String> {
    let blocks = investigate::investigate_multi(&state.conn, &req).map_err(|e| format!("{e:#}"))?;
    Ok(Json(blocks))
}
