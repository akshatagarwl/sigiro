//! End-to-end test for the S3-backed DISTRIBUTED "default" project quack
//! query surface (#12b — GPT-5.6's "Option A": reuse the existing
//! distributed ATTACH path for a single project literally named `default`,
//! no new storage mode). Requires Docker (Postgres catalog + MinIO S3 — see
//! `docker-compose.dev.yml`).
//!
//! Proves the S3/quack refusal previously in `src/server.rs`
//! (`ensure_quack_config_safe`) was safe to remove: with `SIGIRO_S3_BUCKET`
//! set, `pool::lock_filesystem`'s engine-level filesystem lockdown is
//! skipped (DuckLake itself needs network access to reach S3), but
//! `crate::server::validate_read_only` — SELECT-only, `sigiro_*` table
//! allowlist, reader-function denylist, POSITIVE scalar-function allowlist,
//! `WITH`-ban — is exactly the same fail-closed AST firewall the quack
//! authorization gate runs in every storage mode (see
//! `crate::quack::install_authorization_gate`). This test spawns a real
//! `sigiro serve` child process against real Postgres + MinIO (mirroring
//! `tests/quack_query_surface_test.rs`'s patterns: real OTLP/HTTP ingest,
//! `quack_query(...)` client, readiness poll, "Authorization failed"
//! assertions, concurrent ingest, SIGTERM shutdown) with
//! `SIGIRO_CATALOG_URL`/`SIGIRO_S3_*` set, so `"default"` is a DISTRIBUTED
//! project backed by S3 cold storage rather than a local DuckDB file.
//!
//! `#[ignore]`d by default (needs Docker). Run via `just check-s3-e2e`, or
//! directly:
//! ```text
//! docker compose -f docker-compose.dev.yml up -d --wait
//! cargo test --test quack_s3_query_surface_test -- --ignored --nocapture
//! docker compose -f docker-compose.dev.yml down -v
//! ```

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Matches `docker-compose.dev.yml` exactly: `postgres` service
/// (`POSTGRES_USER=sigiro`, `POSTGRES_PASSWORD=sigiro`, `POSTGRES_DB=ducklake`,
/// published on `5432`) and `minio` service (root user/password `minioadmin`,
/// S3 API on `9000`, bucket `sigiro` pre-created by the `minio-setup` job).
const CATALOG_URL: &str = "postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake";
const S3_BUCKET: &str = "sigiro";
const S3_REGION: &str = "us-east-1";
const S3_ENDPOINT: &str = "127.0.0.1:9000";
const S3_KEY_ID: &str = "minioadmin";
const S3_SECRET: &str = "minioadmin";

/// Distinct from the port block `tests/quack_query_surface_test.rs` uses
/// (41100+) so the two test binaries — which `cargo test` runs concurrently
/// by default — never collide on a listener.
const BASE_PORT: u16 = 41300;

/// Shared DuckDB extension cache — see the sibling quack E2E test's doc
/// comment for why (downloads ducklake/sqlite/json/httpfs/quack once, not
/// once per spawned server process).
fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

/// A live `sigiro serve` child process with `SIGIRO_CATALOG_URL` +
/// `SIGIRO_S3_*` set, so the pre-created `"default"` project (see
/// `main.rs run_serve`, which now ALWAYS pre-creates `"default"` when quack
/// is enabled, regardless of storage mode) is a distributed, S3-backed
/// project rather than a local DuckDB file.
struct S3QuackEdge {
    child: Child,
    _data_dir: tempfile::TempDir,
    http_port: u16,
    otlp_http_port: u16,
    quack_port: u16,
    token: String,
}

impl S3QuackEdge {
    /// Spawn a fresh edge on the fixed test port block, with a FRESH temp
    /// `SIGIRO_DATA_DIR` (dropped — and its directory deleted from disk —
    /// when the returned value's `_data_dir` field is dropped, e.g. at the
    /// end of `shutdown()`). Catalog (Postgres) and data (S3/MinIO) are
    /// external Docker services, not per-instance — that is the entire
    /// point of the persistence assertion below.
    fn spawn(token: &str) -> Self {
        let data_dir = tempfile::tempdir().expect("failed to create temp data dir");
        let (http_port, otlp_http_port, grpc_port, quack_port) =
            (BASE_PORT, BASE_PORT + 1, BASE_PORT + 2, BASE_PORT + 3);

        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir.path())
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_CATALOG_URL", CATALOG_URL)
            .env("SIGIRO_S3_BUCKET", S3_BUCKET)
            .env("SIGIRO_S3_REGION", S3_REGION)
            .env("SIGIRO_S3_ENDPOINT", S3_ENDPOINT)
            .env("SIGIRO_S3_KEY_ID", S3_KEY_ID)
            .env("SIGIRO_S3_SECRET", S3_SECRET)
            .env("SIGIRO_S3_USE_SSL", "false")
            .env("SIGIRO_QUERY_SURFACE", "quack")
            .env("SIGIRO_QUACK_BIND", "127.0.0.1")
            .env("SIGIRO_QUACK_PORT", quack_port.to_string())
            .env("SIGIRO_QUACK_TOKEN", token)
            .env("SIGIRO_EXTENSION_DIR", test_extension_dir())
            // Pin the flush cadence: this test waits for ingest visibility on a
            // deadline, so the production 60s default would time it out.
            .env("SIGIRO_FLUSH_INTERVAL_SECS", "1")
            // Empty (not unset) so a developer's ambient OTEL_EXPORTER_OTLP_ENDPOINT
            // never redirects this test's self-instrumentation telemetry elsewhere.
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `sigiro serve`");

        let server = Self {
            child,
            _data_dir: data_dir,
            http_port,
            otlp_http_port,
            quack_port,
            token: token.to_string(),
        };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        // 90s: cold Postgres/MinIO + first-boot extension download can be
        // slow, matching the sibling (local-mode) quack E2E test's budget.
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Ok(resp) =
                reqwest::blocking::get(format!("http://127.0.0.1:{}/health", self.http_port))
                && resp.status().is_success()
                && let Ok(body) = resp.json::<serde_json::Value>()
                && body.get("status").and_then(|v| v.as_str()) == Some("ready")
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "sigiro serve (S3-backed default project) did not report /health ready within 90s"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn ingest_spans(&self, service_name: &str, count: usize) {
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!(
                "http://127.0.0.1:{}/v1/traces",
                self.otlp_http_port
            ))
            .build()
            .expect("failed to build OTLP span exporter");
        let provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name(service_name.to_string())
                    .build(),
            )
            .with_batch_exporter(exporter)
            .build();
        let tracer = provider.tracer("quack-s3-e2e-test");
        for i in 0..count {
            let mut span = tracer.start(format!("op-{i}"));
            span.end();
        }
        provider.shutdown().expect("failed to flush OTLP spans");
    }

    fn quack_select_i64(&self, inner_sql: &str) -> Result<i64, String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("LOAD quack;")
            .map_err(|e| e.to_string())?;
        let wrapped = self.wrap_quack_query(inner_sql);
        conn.query_row(&wrapped, [], |row| row.get::<_, i64>(0))
            .map_err(|e| e.to_string())
    }

    fn quack_exec(&self, inner_sql: &str) -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("LOAD quack;")
            .map_err(|e| e.to_string())?;
        let wrapped = self.wrap_quack_query(inner_sql);
        conn.execute_batch(&wrapped).map_err(|e| e.to_string())
    }

    fn wrap_quack_query(&self, inner_sql: &str) -> String {
        let escaped_inner = inner_sql.replace('\'', "''");
        format!(
            "SELECT * FROM quack_query('quack://127.0.0.1:{}', '{escaped_inner}', token := '{}', disable_ssl := true);",
            self.quack_port, self.token
        )
    }

    /// Poll `count(*)` for `service_name` until it reaches `expected` or a
    /// 20s deadline passes (this test pins the per-project flusher to 1s in
    /// `spawn`; the flush now does an explicit COMMIT, so data should surface
    /// within a flush cycle plus margin — no need to wait past the 30s
    /// CHECKPOINT timer).
    fn wait_for_span_count(&self, service_name: &str, expected: i64) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let sql = format!(
                "SELECT count(*) FROM lake.sigiro_spans WHERE service_name = '{service_name}'"
            );
            let n = self.quack_select_i64(&sql).unwrap_or(0);
            if n >= expected || Instant::now() >= deadline {
                return n;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// SIGTERM, then wait for clean exit. Consumes `self`, so `_data_dir`
    /// (a `tempfile::TempDir`) is dropped — and its directory deleted from
    /// disk — at the end of this call, exactly the "delete local DATA_DIR"
    /// step the persistence assertion below relies on.
    fn shutdown(mut self) -> Result<(), String> {
        // SAFETY: libc::kill on our own child PID with SIGTERM.
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
                        Err(format!(
                            "sigiro serve exited with non-zero/crash status: {status}"
                        ))
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
                        return Err(
                            "sigiro serve did not exit within 15s of SIGTERM — force-killed"
                                .to_string(),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(format!("failed to poll child process status: {e}")),
            }
        }
    }
}

/// A loopback HTTP listener the TEST owns, bound BEFORE the attack list
/// runs. Its only job is to prove a rejected `read_parquet('http://...')`
/// attack never even reaches the network: the authorization gate must
/// reject the query by AST shape (function name on `BLOCKED_TABLE_FUNCTIONS`)
/// before DuckDB ever resolves the URL, so this listener's connection count
/// must stay exactly 0 through the whole attack list.
struct ControlledListener {
    addr: std::net::SocketAddr,
    hits: Arc<AtomicUsize>,
    _accept_thread: std::thread::JoinHandle<()>,
}

impl ControlledListener {
    fn bind() -> Self {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("failed to bind controlled listener");
        let addr = listener.local_addr().expect("failed to read local_addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = Arc::clone(&hits);
        let accept_thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                hits_thread.fetch_add(1, Ordering::SeqCst);
                // This attempt must never happen at all — response content is
                // irrelevant, just record the hit and drop the connection.
                drop(stream);
            }
        });
        Self {
            addr,
            hits,
            _accept_thread: accept_thread,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/exfil-target.parquet", self.addr)
    }

    fn hit_count(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

/// Every read-side attack the quack authorization gate (`validate_read_only`)
/// must reject over the wire, exactly as specified for this test: file/URL
/// readers, credential/introspection functions, a non-sigiro table, `WITH`,
/// and the env/settings scalar-leak hole the positive scalar-function
/// allowlist closes.
fn security_assertions(server: &S3QuackEdge, controlled_url: &str) {
    let attacks: &[(&str, String)] = &[
        (
            "URL read via read_parquet (SSRF/exfil target)",
            format!("SELECT * FROM read_parquet('{controlled_url}')"),
        ),
        (
            "file read via read_csv",
            "SELECT * FROM read_csv('/etc/passwd')".to_string(),
        ),
        (
            "directory listing via glob",
            "SELECT * FROM glob('/etc/*')".to_string(),
        ),
        (
            "credential exfil via duckdb_secrets",
            "SELECT * FROM duckdb_secrets()".to_string(),
        ),
        (
            "non-sigiro table",
            "SELECT * FROM some_other_table".to_string(),
        ),
        (
            "plain WITH banned by policy",
            "WITH x AS (SELECT 1 AS n) SELECT count(*) FROM x".to_string(),
        ),
        (
            "env-var exfil via getenv",
            "SELECT getenv('AWS_SECRET_ACCESS_KEY')".to_string(),
        ),
        (
            "config exfil via current_setting",
            "SELECT current_setting('extension_directory')".to_string(),
        ),
        (
            "env-var exfil hidden inside a DuckLake time-travel AT (...) clause \
             — the AT body must be a literal VERSION/TIMESTAMP, not an arbitrary \
             expression the surrounding SELECT-only validation never sees",
            "SELECT * FROM lake.sigiro_spans AT (TIMESTAMP => (SELECT getenv('X')))".to_string(),
        ),
    ];

    for (label, sql) in attacks {
        let result = server.quack_exec(sql);
        assert!(
            result.is_err(),
            "SECURITY FAILURE: {label} ({sql:?}) was NOT rejected by the quack authorization gate \
             over the S3-backed distributed \"default\" project"
        );
        let message = result.unwrap_err();
        assert!(
            message.contains("Authorization failed"),
            "{label} was rejected, but not by the authorization gate (got: {message}) — \
             a rejection for the wrong reason could mask a real gap"
        );
        println!("  [{label}] rejected: {message}");
    }
}

/// Full lifecycle for the S3-backed distributed "default" project. Returns
/// `Err` describing the first failure so `main` can print a clean summary.
fn run() -> Result<(), String> {
    // quack requires a >= 4 character token.
    let token = "s3-e2e-quack-token".to_string();
    let unique = std::process::id();
    let service_name = format!("quack-s3-e2e-{unique}");

    // 1. Bind the controlled loopback listener BEFORE any attack runs, so a
    //    real network hit (if the gate ever failed open) is guaranteed to be
    //    observed, not missed by a race.
    let controlled = ControlledListener::bind();

    // 2. Spawn the edge — SIGIRO_CATALOG_URL + SIGIRO_S3_* set, so "default"
    //    is pre-created by run_serve as a DISTRIBUTED, S3-backed project.
    let server = S3QuackEdge::spawn(&token);

    // 3. Ingest telemetry over real OTLP/HTTP, read it back over quack.
    const INITIAL_SPANS: usize = 10;
    server.ingest_spans(&service_name, INITIAL_SPANS);
    let seen = server.wait_for_span_count(&service_name, INITIAL_SPANS as i64);
    if seen != INITIAL_SPANS as i64 {
        return server
            .shutdown()
            .map_err(|e| format!("cleanup after count mismatch also failed: {e}"))
            .and(Err(format!(
                "expected {INITIAL_SPANS} spans visible via quack over the S3-backed default \
                 project after ingest, saw {seen}"
            )));
    }
    println!(
        "  [SELECT] {seen} rows visible via quack over the S3-backed distributed \"default\" \
         project — OTLP ingest + quack read both work"
    );

    // 4. Security assertions: every read-side attack must be rejected by the
    //    SAME authorization gate this mode was previously refused for.
    security_assertions(&server, &controlled.url());

    // 5. The controlled listener must have received ZERO requests: a
    //    rejected read_parquet(url) attack must never reach the network.
    let hits = controlled.hit_count();
    if hits != 0 {
        return Err(format!(
            "SECURITY FAILURE: the controlled loopback listener received {hits} request(s) — \
             a read_parquet(url) attack reached the network instead of being rejected before \
             execution"
        ));
    }
    println!(
        "  [controlled listener] 0 requests received — read_parquet(url) attack never reached the network"
    );

    // 6. Nothing mutated: count must be exactly what it was before the attacks.
    let after_attacks = server.quack_select_i64(&format!(
        "SELECT count(*) FROM lake.sigiro_spans WHERE service_name = '{service_name}'"
    ))?;
    if after_attacks != INITIAL_SPANS as i64 {
        return Err(format!(
            "row count changed after rejected attacks: expected {INITIAL_SPANS}, got {after_attacks}"
        ));
    }
    println!("  [count unchanged] {after_attacks} rows after all attack attempts");

    // 7. Concurrent ingest works while quack concurrently serves reads, over
    //    the S3-backed distributed project.
    const CONCURRENT_SPANS: usize = 20;
    let concurrent_service = format!("{service_name}-concurrent");
    let query_errors = std::thread::scope(|scope| {
        let query_handle = scope.spawn(|| {
            let mut errors = 0;
            for _ in 0..15 {
                if server
                    .quack_select_i64("SELECT count(*) FROM lake.sigiro_spans")
                    .is_err()
                {
                    errors += 1;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            errors
        });
        server.ingest_spans(&concurrent_service, CONCURRENT_SPANS);
        query_handle.join().expect("query thread panicked")
    });
    if query_errors > 0 {
        return Err(format!(
            "{query_errors} quack SELECT queries failed while ingest was running concurrently"
        ));
    }
    let concurrent_seen = server.wait_for_span_count(&concurrent_service, CONCURRENT_SPANS as i64);
    if concurrent_seen != CONCURRENT_SPANS as i64 {
        return Err(format!(
            "expected {CONCURRENT_SPANS} concurrently-ingested spans visible via quack, saw {concurrent_seen}"
        ));
    }
    println!(
        "  [concurrent ingest] {concurrent_seen} rows ingested and visible while quack served reads"
    );

    // 8. Graceful shutdown — SIGTERM must stop quack cleanly. Consumes
    //    `server`, dropping (deleting) its temp SIGIRO_DATA_DIR.
    server.shutdown()?;
    println!("  [shutdown] clean exit after SIGTERM; local DATA_DIR deleted");

    // 8b. Assert the codec on the BYTES in S3, not on the set_option call: a
    //     check that the call was issued would still pass if DuckLake accepted
    //     the option and ignored it. A v1 data page cannot carry
    //     DELTA_BINARY_PACKED, so that encoding is positive proof of v2.
    let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.execute_batch(&format!(
        "SET extension_directory = '{}'; INSTALL httpfs; LOAD httpfs; \
         CREATE SECRET (TYPE S3, KEY_ID '{S3_KEY_ID}', SECRET '{S3_SECRET}', \
         REGION '{S3_REGION}', ENDPOINT '{S3_ENDPOINT}', USE_SSL false, URL_STYLE 'path');",
        test_extension_dir()
    ))
    .map_err(|e| e.to_string())?;
    let (files, codecs, delta): (i64, String, bool) = conn
        .query_row(
            "SELECT count(DISTINCT file_name), string_agg(DISTINCT compression, ','), \
                    bool_or(contains(encodings, 'DELTA_BINARY_PACKED')) \
             FROM parquet_metadata(?)",
            [format!(
                "s3://{S3_BUCKET}/default/main/sigiro_spans/*.parquet"
            )],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    if files == 0 || codecs != "ZSTD" || !delta {
        return Err(format!(
            "expected ZSTD + Parquet v2 on every span file in S3, got {files} file(s), \
             codec(s) {codecs:?}, DELTA_BINARY_PACKED present: {delta}"
        ));
    }
    println!("  [parquet] {files} file(s), ZSTD, DELTA_BINARY_PACKED present");

    // 9. Persistence: restart against the SAME Postgres catalog + S3 bucket
    //    (external Docker services) with a BRAND NEW local DATA_DIR (a fresh
    //    tempdir — the old one is gone). If the data survives, it proves the
    //    S3/Postgres pair (not the local disk) is the actual source of truth
    //    for the distributed "default" project.
    let server2 = S3QuackEdge::spawn(&token);
    let after_restart = server2.quack_select_i64(&format!(
        "SELECT count(*) FROM lake.sigiro_spans WHERE service_name = '{service_name}'"
    ))?;
    let restart_result = if after_restart != INITIAL_SPANS as i64 {
        Err(format!(
            "data did not survive restart with a fresh local DATA_DIR: expected {INITIAL_SPANS} \
             rows for {service_name:?} from S3/Postgres, got {after_restart}"
        ))
    } else {
        println!(
            "  [persistence] {after_restart} rows survived a fresh-DATA_DIR restart — S3 + \
             Postgres catalog are confirmed as the source of truth, not local disk"
        );
        Ok(())
    };
    server2
        .shutdown()
        .map_err(|e| format!("cleanup after persistence check also failed: {e}"))?;
    restart_result
}

/// Single-pass S3-backed distributed "default" project quack E2E test.
/// `#[ignore]`d — needs Docker (Postgres + MinIO). Run via
/// `just check-s3-e2e` or:
/// `cargo test --test quack_s3_query_surface_test -- --ignored --nocapture`.
#[test]
#[ignore = "needs Docker (Postgres catalog + MinIO S3) — run via `just check-s3-e2e`"]
fn quack_s3_query_surface_single_iteration() {
    run().expect("S3-backed distributed \"default\" project quack E2E iteration failed");
}
