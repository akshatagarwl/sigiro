//! End-to-end HOSTED tenant-isolation attack matrix. Proves an authenticated
//! tenant cannot read another tenant's PostgreSQL metadata schema or escape the
//! `/v1/query` read-only firewall. Requires Docker Postgres.

use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CATALOG_URL: &str = "postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake";
const CATALOG_LIBPQ: &str = "dbname=ducklake host=127.0.0.1 user=sigiro password=sigiro port=5432";
const BASE_PORT: u16 = 41900;
const ADMIN_TOKEN: &str = "hosted-isolation-e2e-admin-token";
const TENANT_A: &str = "acme";
const TENANT_B: &str = "globex";

fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

fn control_db_exec(sql: &str) {
    let conn = duckdb::Connection::open_in_memory().expect("open in-memory DuckDB for control DB");
    conn.execute_batch(&format!(
        "SET extension_directory = '{}';",
        test_extension_dir().replace('\'', "''")
    ))
    .expect("set extension_directory");
    conn.execute_batch("INSTALL postgres; LOAD postgres;")
        .expect("load postgres extension");
    conn.execute_batch(&format!("ATTACH '{CATALOG_LIBPQ}' AS pg (TYPE postgres)"))
        .expect("attach Postgres control catalog");
    conn.execute_batch(&format!(
        "CALL postgres_execute('pg', '{}')",
        sql.replace('\'', "''")
    ))
    .unwrap_or_else(|e| panic!("control DB exec failed for {sql:?}: {e}"));
}

fn reset_control_db() {
    control_db_exec("DROP SCHEMA IF EXISTS sigiro_control CASCADE");
    control_db_exec(&format!("DROP SCHEMA IF EXISTS tenant_{TENANT_A} CASCADE"));
    control_db_exec(&format!("DROP SCHEMA IF EXISTS tenant_{TENANT_B} CASCADE"));
}

struct Edge {
    child: Child,
    http_port: u16,
    otlp_http_port: u16,
}

impl Edge {
    fn spawn_hosted(data_dir: &Path) -> Self {
        let http_port = BASE_PORT;
        let otlp_http_port = BASE_PORT + 1;
        let grpc_port = BASE_PORT + 2;
        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir)
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_EXTENSION_DIR", test_extension_dir())
            .env("SIGIRO_CATALOG_URL", CATALOG_URL)
            .env("SIGIRO_ADMIN_TOKEN", ADMIN_TOKEN)
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `sigiro serve`");
        let edge = Self {
            child,
            http_port,
            otlp_http_port,
        };
        edge.wait_ready();
        edge
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.http_port)
    }

    fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Ok(resp) = reqwest::blocking::get(format!("{}/health", self.base_url()))
                && resp.status().is_success()
                && let Ok(body) = resp.json::<serde_json::Value>()
                && body.get("status").and_then(|v| v.as_str()) == Some("ready")
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "`sigiro serve` did not report /health ready within 90s"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn create_tenant(&self, id: &str) -> reqwest::StatusCode {
        reqwest::blocking::Client::new()
            .post(format!("{}/admin/tenants", self.base_url()))
            .bearer_auth(ADMIN_TOKEN)
            .json(&serde_json::json!({ "id": id }))
            .send()
            .expect("create_tenant request")
            .status()
    }

    fn mint_key(&self, tenant_id: &str) -> String {
        let body: serde_json::Value = reqwest::blocking::Client::new()
            .post(format!("{}/admin/keys", self.base_url()))
            .bearer_auth(ADMIN_TOKEN)
            .json(&serde_json::json!({ "tenant_id": tenant_id }))
            .send()
            .expect("mint_key request")
            .error_for_status()
            .expect("mint_key non-2xx")
            .json()
            .expect("mint_key body");
        body["key"].as_str().expect("key field").to_string()
    }

    fn ingest_spans(&self, key: &str, service_name: &str, count: usize) {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!(
                "http://127.0.0.1:{}/v1/traces",
                self.otlp_http_port
            ))
            .with_headers(headers)
            .build()
            .expect("build OTLP span exporter");
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name(service_name.to_string())
                    .build(),
            )
            .with_batch_exporter(exporter)
            .build();
        let tracer = provider.tracer("hosted-isolation-e2e");
        for i in 0..count {
            let mut span = tracer.start(format!("op-{i}"));
            span.end();
        }
        provider.shutdown().expect("flush OTLP spans");
    }

    fn query_count(&self, key: &str, service_name: &str) -> Result<i64, reqwest::StatusCode> {
        let sql = format!(
            "SELECT count(*) AS n FROM lake.sigiro_spans WHERE service_name = '{service_name}'"
        );
        let resp = reqwest::blocking::Client::new()
            .post(format!("{}/v1/query", self.base_url()))
            .bearer_auth(key)
            .body(sql)
            .send()
            .expect("query request");
        if !resp.status().is_success() {
            return Err(resp.status());
        }
        let rows: Vec<serde_json::Value> = resp.json().expect("query body");
        Ok(rows
            .first()
            .and_then(|r| r.get("n"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0))
    }

    fn wait_for_count(&self, key: &str, service_name: &str, expected: i64) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let n = self.query_count(key, service_name).unwrap_or(0);
            if n >= expected || Instant::now() >= deadline {
                return n;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn shutdown(mut self) -> Result<(), String> {
        let pid = self.child.id() as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(format!("`sigiro serve` exited non-zero: {status}"))
                    };
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        if let Err(e) = self.child.kill() {
                            println!("  (cleanup: force-kill after timeout failed: {e})");
                        }
                        if let Err(e) = self.child.wait() {
                            println!("  (cleanup: wait after force-kill failed: {e})");
                        }
                        return Err("`sigiro serve` did not exit within 15s of SIGTERM".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(format!("failed to poll child status: {e}")),
            }
        }
    }
}

#[test]
#[ignore = "needs Docker (Postgres control catalog) — run via `just check-hosted-isolation-e2e`"]
fn hosted_tenant_isolation_attacks() {
    reset_control_db();

    let data_dir = tempfile::tempdir().expect("temp data dir");
    let edge = Edge::spawn_hosted(data_dir.path());
    assert!(
        edge.create_tenant(TENANT_A).is_success(),
        "create {TENANT_A}"
    );
    assert!(
        edge.create_tenant(TENANT_B).is_success(),
        "create {TENANT_B}"
    );
    let acme_key = edge.mint_key(TENANT_A);
    let globex_key = edge.mint_key(TENANT_B);
    let acme_service = format!("acme-svc-{}", std::process::id());
    let globex_service = format!("globex-svc-{}", std::process::id());

    const SPANS: usize = 3;
    edge.ingest_spans(&acme_key, &acme_service, SPANS);
    edge.ingest_spans(&globex_key, &globex_service, SPANS);
    assert_eq!(
        edge.wait_for_count(&acme_key, &acme_service, SPANS as i64),
        SPANS as i64,
        "positive control: acme can read its own spans"
    );
    assert_eq!(
        edge.wait_for_count(&globex_key, &globex_service, SPANS as i64),
        SPANS as i64,
        "globex ingest must be visible before isolation attacks run"
    );

    let attacks: &[(&str, &str, bool)] = &[
        (
            "direct globex metadata schema",
            "SELECT * FROM tenant_globex.sigiro_spans",
            true,
        ),
        (
            "lake-qualified globex metadata schema",
            "SELECT * FROM lake.tenant_globex.sigiro_spans",
            true,
        ),
        (
            "ATTACH another catalog",
            "ATTACH ':memory:' AS escaped",
            true,
        ),
        (
            "local parquet reader",
            "SELECT * FROM read_parquet('/tmp/globex.parquet')",
            true,
        ),
        ("CSV reader", "SELECT * FROM read_csv('/etc/passwd')", true),
        ("directory glob", "SELECT * FROM glob('/etc/*')", true),
        (
            "guessed globex S3 prefix",
            "SELECT * FROM read_parquet('s3://sigiro/globex/*.parquet')",
            true,
        ),
        (
            "environment exfiltration",
            "SELECT getenv('AWS_SECRET_ACCESS_KEY')",
            true,
        ),
        (
            "configuration exfiltration",
            "SELECT current_setting('extension_directory')",
            true,
        ),
        (
            "CTE cross-tenant bypass",
            "WITH stolen AS (SELECT * FROM tenant_globex.sigiro_spans) SELECT * FROM stolen",
            true,
        ),
        (
            "subquery cross-tenant bypass",
            "SELECT * FROM (SELECT * FROM tenant_globex.sigiro_spans) AS stolen",
            true,
        ),
        (
            "non-literal time travel",
            "SELECT * FROM lake.sigiro_spans AT (VERSION => 1 + 1)",
            true,
        ),
        // Shared control-plane theft — the highest-value target: sigiro_control
        // holds EVERY tenant's key hash + tenant rows. A tenant query must never
        // reach it (not attached to the tenant connection + firewall rejects the
        // unrecognized schema / non-allowlisted table).
        (
            "control-plane api_keys read (all tenants' key hashes)",
            "SELECT * FROM sigiro_control.api_keys",
            true,
        ),
        (
            "control-plane tenants read",
            "SELECT * FROM sigiro_control.tenants",
            true,
        ),
        // Postgres-extension escape to run arbitrary SQL on the shared catalog.
        (
            "postgres_query escape to control plane",
            "SELECT * FROM postgres_query('pg', 'SELECT key_hash FROM sigiro_control.api_keys')",
            true,
        ),
        (
            "postgres_execute escape",
            "CALL postgres_execute('pg', 'SELECT 1')",
            true,
        ),
        // System-catalog enumeration that returns ROWS (so must be firewall-
        // rejected, not merely empty) — pg_namespace lists every PG schema.
        (
            "pg_catalog.pg_namespace enumeration",
            "SELECT nspname FROM pg_catalog.pg_namespace",
            true,
        ),
        // Set-operation / join arms the AST walker must cover, not just CTE/derived.
        (
            "UNION cross-tenant traversal",
            "SELECT service_name FROM lake.sigiro_spans UNION ALL SELECT service_name FROM tenant_globex.sigiro_spans",
            true,
        ),
        (
            "JOIN cross-tenant traversal",
            "SELECT a.service_name FROM lake.sigiro_spans a JOIN tenant_globex.sigiro_spans b ON true",
            true,
        ),
        // Write/DDL — the firewall is query-only; these must be rejected.
        (
            "write into globex schema",
            "INSERT INTO tenant_globex.sigiro_spans VALUES (now(), 'x')",
            true,
        ),
        (
            "CREATE in globex schema",
            "CREATE TABLE tenant_globex.stolen AS SELECT 1",
            true,
        ),
        // Multi-statement: only a single statement is allowed.
        (
            "multi-statement injection",
            "SELECT 1; SELECT * FROM tenant_globex.sigiro_spans",
            true,
        ),
    ];

    for (label, sql, expect_rejected) in attacks {
        let resp = reqwest::blocking::Client::new()
            .post(format!("{}/v1/query", edge.base_url()))
            .bearer_auth(&acme_key)
            .body(*sql)
            .send()
            .unwrap_or_else(|e| panic!("{label}: query request failed: {e}"));
        if resp.status().is_success() {
            let rows: Vec<serde_json::Value> = resp
                .json()
                .unwrap_or_else(|e| panic!("{label}: successful response was not JSON rows: {e}"));
            assert!(rows.is_empty(), "{label}: attack returned rows: {rows:?}");
            assert!(
                !rows
                    .iter()
                    .any(|row| row.to_string().contains(&globex_service)),
                "{label}: acme received globex data: {rows:?}"
            );
        } else {
            assert!(
                resp.status().is_client_error(),
                "{label}: expected a 4xx firewall/authorization rejection (expect_rejected={expect_rejected}), got {}",
                resp.status()
            );
        }
    }

    // Metadata enumeration via information_schema. In HOSTED mode this is
    // firewall-rejected (see validate_read_only_with — a shared PG catalog would
    // otherwise expose the whole DB's information_schema), so the response must
    // name NEITHER another tenant's data schema (`tenant_globex`) NOR the shared
    // control plane (`sigiro_control`), whether the query 4xx-rejects or returns
    // only acme's own schema.
    for (label, sql) in [
        (
            "information_schema.tables",
            "SELECT table_schema, table_name FROM information_schema.tables",
        ),
        (
            "information_schema.schemata",
            "SELECT schema_name FROM information_schema.schemata",
        ),
        (
            "duckdb_tables()",
            "SELECT schema_name, table_name FROM duckdb_tables()",
        ),
        (
            "duckdb_databases()",
            "SELECT database_name FROM duckdb_databases()",
        ),
    ] {
        let body = reqwest::blocking::Client::new()
            .post(format!("{}/v1/query", edge.base_url()))
            .bearer_auth(&acme_key)
            .body(sql)
            .send()
            .unwrap_or_else(|e| panic!("{label}: request failed: {e}"))
            .text()
            .unwrap_or_default();
        assert!(
            !body.contains("tenant_globex") && !body.contains("sigiro_control"),
            "{label}: enumeration leaked another tenant's schema or the control plane: {body}"
        );
    }

    assert_eq!(
        edge.query_count(&acme_key, &globex_service).unwrap(),
        0,
        "acme must never see globex spans through the allowed lake view"
    );
    edge.shutdown().expect("hosted edge shutdown");
    reset_control_db();
}
