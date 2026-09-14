//! End-to-end HOSTED CONTROL-PLANE test. Proves the multi-tenant provisioning
//! and auth lifecycle works against a REAL shared PostgreSQL control catalog
//! (`sigiro_control`), driven through the admin HTTP endpoints and the
//! per-tenant Bearer-key ingest/query surface — how a hosted operator and its
//! tenants actually use sigiro.
//!
//! This is the `query_surface == None` / `TenantKeys` auth mode (NOT the quack
//! edge, which stays on static/open auth). Storage is PostgreSQL catalog + LOCAL
//! Parquet, so only Postgres is required (no RustFS); the control-plane behavior
//! is orthogonal to where data files live.
//!
//! Coverage:
//!   - create 2 tenants via `POST /admin/tenants`;
//!   - REJECT a hosted tenant literally named `default` (fail-loud, non-2xx);
//!   - mint MULTIPLE keys per tenant via `POST /admin/keys`;
//!   - authenticate an INGEST (OTLP HTTP, `Authorization: Bearer <key>`) and a
//!     QUERY (`POST /v1/query`) with a tenant key;
//!   - REVOKE one key (`DELETE /admin/keys/{hash_prefix}`) — subsequent ingest
//!     AND query with THAT key fail, while a sibling key of the same tenant
//!     still works;
//!   - DEACTIVATE a tenant (control-DB `status='suspended'`) — its keys stop
//!     resolving — then REACTIVATE via `POST /admin/tenants` and confirm they
//!     resolve again;
//!   - RESTART two server processes against the SAME PG control schema + data
//!     dir and confirm no duplicate/ambiguous tenant/key state (idempotent
//!     create keeps exactly one row; a surviving key still authenticates).
//!   - Plus: zero-config LOCAL startup still pre-creates + queries `default`.
//!
//! `#[ignore]`d — needs Docker (Postgres). Run via `mise run check-control-plane-e2e`,
//! or directly:
//! ```text
//! docker compose -f docker-compose.dev.yml up -d --wait postgres
//! cargo test --test control_plane_e2e_test -- --ignored --nocapture
//! docker compose -f docker-compose.dev.yml down -v
//! ```

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Matches `docker-compose.dev.yml` (see the sibling storage E2Es).
const CATALOG_URL: &str = "postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake";
/// libpq form of the above, for the control-DB reset/suspend DuckDB->PG attach.
const CATALOG_LIBPQ: &str = "dbname=ducklake host=127.0.0.1 user=sigiro password=sigiro port=5432";

/// Distinct from the port blocks the other E2Es use (41100+, 41300+, 41500+).
const BASE_PORT: u16 = 41700;
const ADMIN_TOKEN: &str = "control-plane-e2e-admin-token";

/// The two tenants provisioned by the test + the reserved name that must be
/// rejected. Metadata schemas `tenant_<id>` are dropped by `reset_control_db`.
const TENANT_A: &str = "acme";
const TENANT_B: &str = "globex";

/// Shared DuckDB extension cache — see the sibling quack E2E's doc comment.
fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

/// Run one statement against the shared Postgres catalog via DuckDB's `postgres`
/// extension (exactly as `auth::ControlPlane` attaches it). Used for the
/// out-of-band control-DB operations an operator would perform directly:
/// resetting between runs, suspending a tenant, and asserting row counts.
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

/// Count rows in the control catalog matching a WHERE clause — the duplicate/
/// ambiguity assertion after restarts.
fn control_db_count(table: &str, where_clause: &str) -> i64 {
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
    let inner = format!("SELECT count(*)::BIGINT FROM sigiro_control.{table} WHERE {where_clause}");
    let wrapped = format!(
        "SELECT * FROM postgres_query('pg', '{}')",
        inner.replace('\'', "''")
    );
    conn.query_row(&wrapped, [], |row| row.get::<_, i64>(0))
        .unwrap_or_else(|e| panic!("control DB count failed for {inner:?}: {e}"))
}

/// Drop all control-plane + per-tenant state so each run starts clean.
fn reset_control_db() {
    control_db_exec("DROP SCHEMA IF EXISTS sigiro_control CASCADE");
    control_db_exec(&format!("DROP SCHEMA IF EXISTS tenant_{TENANT_A} CASCADE"));
    control_db_exec(&format!("DROP SCHEMA IF EXISTS tenant_{TENANT_B} CASCADE"));
}

/// A live `sigiro serve` child. Data dir is owned by the caller so it can be
/// reused across a restart.
struct Edge {
    child: Child,
    http_port: u16,
    otlp_http_port: u16,
}

impl Edge {
    /// Spawn a HOSTED (distributed PG catalog, local data, TenantKeys auth)
    /// server. No quack surface, so auth is control-plane per-tenant keys.
    fn spawn_hosted(data_dir: &Path) -> Self {
        Self::spawn(data_dir, true)
    }

    /// Spawn a zero-config LOCAL server (no catalog URL, Open auth).
    fn spawn_local(data_dir: &Path) -> Self {
        Self::spawn(data_dir, false)
    }

    fn spawn(data_dir: &Path, hosted: bool) -> Self {
        // Hosted + local use disjoint port blocks so a leftover local server
        // never collides with the hosted one.
        let offset = if hosted { 0 } else { 10 };
        let http_port = BASE_PORT + offset;
        let otlp_http_port = BASE_PORT + offset + 1;
        let grpc_port = BASE_PORT + offset + 2;

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigiro"));
        cmd.arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir)
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_EXTENSION_DIR", test_extension_dir())
            // Pin the flush cadence: this test waits for ingest visibility on a
            // deadline, so the production 60s default would time it out.
            .env("SIGIRO_FLUSH_INTERVAL_SECS", "1")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if hosted {
            cmd.env("SIGIRO_CATALOG_URL", CATALOG_URL)
                .env("SIGIRO_ADMIN_TOKEN", ADMIN_TOKEN);
        }

        let child = cmd.spawn().expect("failed to spawn `sigiro serve`");
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

    // ---- admin (control-plane) HTTP surface ----

    /// `POST /admin/tenants` — returns the HTTP status so callers can assert
    /// both success and fail-loud rejection.
    fn create_tenant(&self, id: &str) -> reqwest::StatusCode {
        reqwest::blocking::Client::new()
            .post(format!("{}/admin/tenants", self.base_url()))
            .bearer_auth(ADMIN_TOKEN)
            .json(&serde_json::json!({ "id": id }))
            .send()
            .expect("create_tenant request")
            .status()
    }

    /// `POST /admin/keys` returning the raw HTTP status — for asserting that
    /// minting against a nonexistent/suspended tenant is a deterministic CLIENT
    /// error (4xx), not a 500. (A 500 would also be "not success", so the
    /// success/failure helpers below can't tell the two apart.)
    fn mint_key_status(&self, tenant_id: &str) -> reqwest::StatusCode {
        reqwest::blocking::Client::new()
            .post(format!("{}/admin/keys", self.base_url()))
            .bearer_auth(ADMIN_TOKEN)
            .json(&serde_json::json!({ "tenant_id": tenant_id }))
            .send()
            .expect("mint_key request")
            .status()
    }

    /// `POST /admin/keys` — mints a key for an existing tenant; returns
    /// `(raw_key, key_hash)`.
    fn mint_key(&self, tenant_id: &str) -> (String, String) {
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
        let key = body["key"].as_str().expect("key field").to_string();
        let key_hash = body["key_hash"]
            .as_str()
            .expect("key_hash field")
            .to_string();
        (key, key_hash)
    }

    /// `DELETE /admin/keys/{hash_prefix}` — returns whether a key was revoked.
    fn revoke_key(&self, key_hash: &str) -> bool {
        let prefix = &key_hash[..16];
        let body: serde_json::Value = reqwest::blocking::Client::new()
            .delete(format!("{}/admin/keys/{prefix}", self.base_url()))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .expect("revoke_key request")
            .error_for_status()
            .expect("revoke_key non-2xx")
            .json()
            .expect("revoke_key body");
        assert_eq!(
            body["short_token"], prefix,
            "hash-prefix wire shape changed"
        );
        body["revoked"].as_bool().expect("revoked field")
    }

    // ---- tenant data surface (Bearer <tenant key>) ----

    /// Ingest `count` spans via OTLP HTTP authenticated with a tenant key.
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
        let tracer = provider.tracer("control-plane-e2e");
        for i in 0..count {
            let mut span = tracer.start(format!("op-{i}"));
            span.end();
        }
        provider.shutdown().expect("flush OTLP spans");
    }

    /// `POST /v1/query` with a tenant key. Returns `Ok(count)` on 2xx, or the
    /// HTTP status string on failure (so revoked/suspended keys can be asserted
    /// to FAIL rather than silently returning 0).
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

    /// Poll `/v1/query` until the expected span count is visible (post-flush).
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

    /// Assert an INGEST authenticated with `key` is rejected (401). We drive the
    /// OTLP HTTP endpoint directly (not the exporter, which swallows the status)
    /// so we can observe the unauthorized response.
    fn ingest_rejected(&self, key: &str) -> bool {
        let status = reqwest::blocking::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/v1/traces",
                self.otlp_http_port
            ))
            .bearer_auth(key)
            .header("content-type", "application/x-protobuf")
            .body(Vec::new())
            .send()
            .expect("ingest request")
            .status();
        status == reqwest::StatusCode::UNAUTHORIZED
    }

    fn shutdown(mut self) -> Result<(), String> {
        let pid = self.child.id() as libc::pid_t;
        // SAFETY: SIGTERM to our own child PID.
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
#[ignore = "needs Docker (Postgres control catalog) — run via `mise run check-control-plane-e2e`"]
fn control_plane_lifecycle() {
    reset_control_db();

    let data_dir = tempfile::tempdir().expect("temp data dir");
    let svc_a = format!("cp-a-{}", std::process::id());
    let svc_b = format!("cp-b-{}", std::process::id());

    let edge = Edge::spawn_hosted(data_dir.path());

    // --- provisioning: two tenants, reserved "default" rejected ---
    assert!(
        edge.create_tenant(TENANT_A).is_success(),
        "creating tenant {TENANT_A} should succeed"
    );
    assert!(
        edge.create_tenant(TENANT_B).is_success(),
        "creating tenant {TENANT_B} should succeed"
    );
    let default_status = edge.create_tenant("default");
    assert!(
        default_status.is_client_error(),
        "hosted control plane must REJECT tenant named 'default' with a 4xx (deterministic \
         client error), not a 5xx — got {default_status}"
    );
    // Minting a key for a tenant that was never created is a deterministic
    // client error (404), NOT a 500.
    let ghost_status = edge.mint_key_status("ghost_tenant");
    assert!(
        ghost_status.is_client_error(),
        "minting a key for a nonexistent tenant must be a 4xx, got {ghost_status}"
    );
    println!(
        "  provisioned {TENANT_A} + {TENANT_B}; 'default' rejected ({default_status}); \
         nonexistent-tenant mint rejected ({ghost_status})"
    );

    // --- multiple keys per tenant ---
    let (a_key1, a_hash1) = edge.mint_key(TENANT_A);
    let (a_key2, _a_hash2) = edge.mint_key(TENANT_A);
    let (b_key1, _b_hash1) = edge.mint_key(TENANT_B);
    assert_ne!(a_key1, a_key2, "each minted key must be distinct");
    println!("  minted 2 keys for {TENANT_A}, 1 for {TENANT_B}");

    // --- authenticated ingest + query with a tenant key ---
    const SPANS: usize = 10;
    edge.ingest_spans(&a_key1, &svc_a, SPANS);
    let seen = edge.wait_for_count(&a_key1, &svc_a, SPANS as i64);
    assert_eq!(seen, SPANS as i64, "tenant {TENANT_A} ingest+query via key");

    // tenant isolation: B's key sees none of A's data.
    edge.ingest_spans(&b_key1, &svc_b, SPANS);
    edge.wait_for_count(&b_key1, &svc_b, SPANS as i64);
    assert_eq!(
        edge.query_count(&b_key1, &svc_a).unwrap(),
        0,
        "tenant {TENANT_B} must NOT see tenant {TENANT_A} spans"
    );
    println!("  authenticated ingest+query OK ({seen} spans); cross-tenant isolation holds");

    // --- revoke one key; sibling key of the same tenant still works ---
    assert!(
        edge.revoke_key(&a_hash1),
        "revoke should report a key removed"
    );
    assert!(
        edge.ingest_rejected(&a_key1),
        "ingest with revoked key must be 401"
    );
    assert_eq!(
        edge.query_count(&a_key1, &svc_a),
        Err(reqwest::StatusCode::UNAUTHORIZED),
        "query with revoked key must be 401"
    );
    assert_eq!(
        edge.query_count(&a_key2, &svc_a).unwrap(),
        SPANS as i64,
        "sibling key of the same tenant must still authenticate"
    );
    println!("  revoked {TENANT_A} key1: it now 401s; sibling key2 still works");

    // --- deactivate then reactivate a tenant ---
    control_db_exec(&format!(
        "UPDATE sigiro_control.tenants SET status = 'suspended' WHERE id = '{TENANT_B}'"
    ));
    assert_eq!(
        edge.query_count(&b_key1, &svc_b),
        Err(reqwest::StatusCode::UNAUTHORIZED),
        "suspended tenant's key must stop resolving"
    );
    // Minting a NEW key for a suspended tenant is a deterministic client error
    // (404), NOT a 500.
    let suspended_mint = edge.mint_key_status(TENANT_B);
    assert!(
        suspended_mint.is_client_error(),
        "minting a key for a suspended tenant must be a 4xx, got {suspended_mint}"
    );
    // Reactivate through the create-or-reactivate admin endpoint.
    assert!(edge.create_tenant(TENANT_B).is_success());
    assert_eq!(
        edge.query_count(&b_key1, &svc_b).unwrap(),
        SPANS as i64,
        "reactivated tenant's key must resolve again"
    );
    println!("  {TENANT_B} suspended -> key 401s -> reactivated -> key works");

    edge.shutdown().expect("hosted edge shutdown");

    // --- restart against the SAME PG control schema + data dir ---
    let edge2 = Edge::spawn_hosted(data_dir.path());

    // Idempotent create: re-creating an existing tenant twice must NOT
    // duplicate rows.
    assert!(edge2.create_tenant(TENANT_A).is_success());
    assert!(edge2.create_tenant(TENANT_A).is_success());
    assert_eq!(
        control_db_count("tenants", &format!("id = '{TENANT_A}'")),
        1,
        "idempotent create must leave exactly one tenant row"
    );
    // Surviving (unrevoked) key still authenticates + prior data persisted.
    // Projects are materialized lazily on ingest, so a fresh ingest with the
    // surviving key re-attaches tenant acme's DuckLake catalog in this new
    // process; once attached, the spans written BEFORE the restart (PG catalog
    // + local data are the source of truth) are queryable again.
    let svc_a_restart = format!("{svc_a}-restart");
    edge2.ingest_spans(&a_key2, &svc_a_restart, SPANS);
    edge2.wait_for_count(&a_key2, &svc_a_restart, SPANS as i64);
    assert_eq!(
        edge2.query_count(&a_key2, &svc_a).unwrap(),
        SPANS as i64,
        "spans ingested before the restart must persist under the surviving key"
    );
    // The revoked key stays revoked across the restart (rejected at auth, 401).
    assert_eq!(
        edge2.query_count(&a_key1, &svc_a),
        Err(reqwest::StatusCode::UNAUTHORIZED),
        "revoked key must stay revoked after restart"
    );
    // No ambiguous key state: exactly one live key remains for TENANT_A.
    assert_eq!(
        control_db_count(
            "api_keys",
            &format!("tenant_id = '{TENANT_A}' AND revoked_at IS NULL")
        ),
        1,
        "exactly one unrevoked key should remain for {TENANT_A}"
    );
    println!("  restart OK — no duplicate/ambiguous tenant/key state; revocation persisted");

    edge2.shutdown().expect("restart edge shutdown");

    // --- zero-config LOCAL startup still creates + queries "default" ---
    let local_dir = tempfile::tempdir().expect("temp local data dir");
    let local = Edge::spawn_local(local_dir.path());
    let svc_local = format!("cp-local-{}", std::process::id());
    // Open auth: no Bearer key needed; tenant resolves to "default".
    local.ingest_spans("", &svc_local, SPANS);
    let local_seen = local.wait_for_count("", &svc_local, SPANS as i64);
    assert_eq!(
        local_seen, SPANS as i64,
        "zero-config LOCAL startup must create + query the 'default' tenant"
    );
    local.shutdown().expect("local edge shutdown");
    println!("  zero-config LOCAL 'default' tenant ingest+query OK ({local_seen} spans)");

    println!("control-plane lifecycle E2E passed");
}
