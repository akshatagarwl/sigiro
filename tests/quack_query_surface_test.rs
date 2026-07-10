//! End-to-end test for the quack read-only edge query-serving surface
//! (`SIGIRO_QUERY_SURFACE=quack`).
//!
//! Unlike this repo's other integration tests (which build a `ProjectPool`/
//! `AppState` in-process and drive the axum `Router` directly), this test
//! spawns the REAL `sigiro serve` binary as a child process. That is
//! deliberate: the thing under test is the production boot sequence itself —
//! extension load + version pin, authorization-gate install, in-process
//! `quack_serve` on a connection cloned from the same DuckDB instance ingest
//! writes to, and a clean `quack_stop`-based shutdown on SIGTERM — none of
//! which exists on the in-process `AppState` test path.
//!
//! Client side: connects via `quack_query(...)` (a direct remote-exec RPC),
//! not `ATTACH ... (TYPE quack)`. Investigated directly: `ATTACH` completes
//! the full quack wire handshake successfully server-side (confirmed via
//! `duckdb_logs()` — both handshake `PREPARE_REQUEST`s return
//! `PREPARE_RESPONSE` with no error) but this specific duckdb/quack build
//! then fails client-side with `Binder Error: Catalog "remote" does not
//! exist!` while registering the attached catalog locally — a client-library
//! quirk unrelated to sigiro's authorization gate. `quack_query` is the
//! documented alternative ("ATTACH ... and/or quack_query(...)") and
//! exercises the exact same server-side authorization path.
//!
//! Run with: `cargo test --release --test quack_query_surface_test -- --nocapture`
//! (add `--ignored` to include the 10-iteration stability loop, which is
//! marked `#[ignore]` by default because it spawns 10 full server processes
//! and takes on the order of a minute).

use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Shared DuckDB extension cache — matches the convention already used by
/// `tests/e2e_test.rs`/`tests/pyroscope_e2e_test.rs` so ducklake/sqlite/json/
/// quack extensions download once across the whole test run, not once per
/// spawned server process.
fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

/// A live `sigiro serve` child process configured with
/// `SIGIRO_QUERY_SURFACE=quack`, plus everything needed to talk to it.
struct QuackEdgeServer {
    child: Child,
    _data_dir: tempfile::TempDir,
    http_port: u16,
    otlp_http_port: u16,
    quack_port: u16,
    token: String,
}

impl QuackEdgeServer {
    /// Spawn a fresh server on a distinct port block (`iteration` selects the
    /// block so repeated-loop iterations never collide) and block until
    /// `/health` reports ready.
    fn spawn(iteration: u16) -> Self {
        let data_dir = tempfile::tempdir().expect("failed to create temp data dir");
        let base = 41100 + iteration * 10;
        let (http_port, otlp_http_port, grpc_port, quack_port) =
            (base, base + 1, base + 2, base + 3);
        // quack requires a >= 4 character token.
        let token = format!("e2e-quack-token-{iteration:04}");

        let child = Command::new(env!("CARGO_BIN_EXE_sigiro"))
            .arg("serve")
            .env("SIGIRO_DATA_DIR", data_dir.path())
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_QUERY_SURFACE", "quack")
            .env("SIGIRO_QUACK_BIND", "127.0.0.1")
            .env("SIGIRO_QUACK_PORT", quack_port.to_string())
            .env("SIGIRO_QUACK_TOKEN", &token)
            .env("SIGIRO_EXTENSION_DIR", test_extension_dir())
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
            token,
        };
        server.wait_ready();
        server
    }

    fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
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
                "sigiro serve did not report /health ready within 30s"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Ingest `count` spans for `service_name` via real OTLP/HTTP export —
    /// the actual wire protocol, not a direct pool/DB write.
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
        let tracer = provider.tracer("quack-e2e-test");
        for i in 0..count {
            let mut span = tracer.start(format!("op-{i}"));
            span.end();
        }
        provider.shutdown().expect("failed to flush OTLP spans");
    }

    /// Run `inner_sql` against this server over the quack wire protocol via
    /// `quack_query`, returning the first column of the first row as i64.
    fn quack_select_i64(&self, inner_sql: &str) -> Result<i64, String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("LOAD quack;")
            .map_err(|e| e.to_string())?;
        let wrapped = self.wrap_quack_query(inner_sql);
        conn.query_row(&wrapped, [], |row| row.get::<_, i64>(0))
            .map_err(|e| e.to_string())
    }

    /// Run `inner_sql` against this server over the quack wire protocol via
    /// `quack_query`, returning whether it was accepted (authorization +
    /// execution both succeeded) or rejected.
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
    /// 10s deadline passes (the per-project flusher default-flushes every
    /// 1s, so ingested rows are not visible instantly).
    fn wait_for_span_count(&self, service_name: &str, expected: i64) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(10);
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

    /// SIGTERM, then wait for the process to exit cleanly. Returns `Ok(())`
    /// if the process exited with a success status within the deadline;
    /// otherwise force-kills it and returns an error describing what
    /// happened (a hang or a non-zero/crash exit — either would mean the
    /// in-process `quack_stop`-before-drop shutdown path is not clean).
    fn shutdown(mut self) -> Result<(), String> {
        // SAFETY: libc::kill on our own child PID with SIGTERM — the same
        // graceful-shutdown signal src/server.rs already listens for.
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

/// Every write/DDL/DCL form the quack authorization gate must reject over
/// the wire, alongside the real sigiro table it targets.
fn security_assertions(server: &QuackEdgeServer, service_name: &str) {
    let attacks: &[(&str, String)] = &[
        (
            "CREATE TABLE",
            "CREATE TABLE lake.pwned AS SELECT 42".to_string(),
        ),
        (
            "INSERT",
            format!(
                "INSERT INTO lake.sigiro_spans (trace_id, service_name) VALUES ('injected', '{service_name}')"
            ),
        ),
        ("DELETE", "DELETE FROM lake.sigiro_spans".to_string()),
        ("DROP TABLE", "DROP TABLE lake.sigiro_spans".to_string()),
        ("ATTACH", "ATTACH ':memory:' AS evil".to_string()),
        (
            "COPY",
            "COPY lake.sigiro_spans TO '/tmp/sigiro-quack-e2e-exfil.csv'".to_string(),
        ),
        ("PRAGMA/SET", "SET memory_limit = '999MB'".to_string()),
    ];

    for (label, sql) in attacks {
        let result = server.quack_exec(sql);
        assert!(
            result.is_err(),
            "SECURITY FAILURE: {label} ({sql:?}) was NOT rejected by the quack authorization gate"
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

/// One full lifecycle: spawn, ingest, verify read, run every security
/// assertion, verify count unchanged, verify concurrent ingest still works,
/// then shut down cleanly. Returns `Err` describing the first failure.
fn run_one_iteration(iteration: u16) -> Result<(), String> {
    let service_name = format!("quack-e2e-{iteration}");
    println!("--- iteration {iteration} ({service_name}) ---");

    let server = QuackEdgeServer::spawn(iteration);

    // 1. Ingest telemetry over real OTLP/HTTP.
    const INITIAL_SPANS: usize = 10;
    server.ingest_spans(&service_name, INITIAL_SPANS);
    let seen = server.wait_for_span_count(&service_name, INITIAL_SPANS as i64);
    if seen != INITIAL_SPANS as i64 {
        return server
            .shutdown()
            .map_err(|e| format!("cleanup after count mismatch also failed: {e}"))
            .and(Err(format!(
                "expected {INITIAL_SPANS} spans visible via quack after ingest, saw {seen}"
            )));
    }
    println!("  [SELECT] {seen} rows visible via quack — OTLP ingest + quack read both work");

    // 2. Security assertions: every write form must be rejected, and the
    //    gate's own denial (not some other coincidental failure) must be why.
    security_assertions(&server, &service_name);

    // 3. Nothing mutated: count must be exactly what it was before the attacks.
    let after_attacks = server.quack_select_i64(&format!(
        "SELECT count(*) FROM lake.sigiro_spans WHERE service_name = '{service_name}'"
    ))?;
    if after_attacks != INITIAL_SPANS as i64 {
        return Err(format!(
            "row count changed after rejected writes: expected {INITIAL_SPANS}, got {after_attacks} \
             — a rejected statement must never partially apply"
        ));
    }
    println!(
        "  [count unchanged] {after_attacks} rows after all {} attack attempts",
        7
    );

    // 4. Ingest continues to work WHILE quack is concurrently serving reads —
    //    the whole point of in-process serving on a cloned connection rather
    //    than a separate reader-locked process.
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
            "{query_errors} quack SELECT queries failed while ingest was running concurrently \
             in-process"
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

    // 5. Graceful shutdown: SIGTERM must stop quack (quack_stop) and exit
    //    cleanly, not hang or abort the process.
    server.shutdown()?;
    println!("  [shutdown] clean exit after SIGTERM");

    Ok(())
}

/// Single-pass smoke test — always runs (not `#[ignore]`d) as part of the
/// standard `cargo test` gate.
#[test]
fn quack_query_surface_single_iteration() {
    run_one_iteration(0).expect("quack query-surface E2E iteration failed");
}

/// Repeats the full lifecycle 10 times end-to-end (fresh process boot →
/// ingest → quack read → security assertions → concurrent ingest → SIGTERM
/// shutdown) to shake out flakiness/stability of the in-process quack serve
/// and its shutdown path, as specifically requested. `#[ignore]`d by default
/// (spawns 10 full server processes, ~1 minute) — run explicitly with
/// `cargo test --release --test quack_query_surface_test -- --ignored --nocapture`.
#[test]
#[ignore = "spawns 10 full sigiro server processes; run explicitly for the stability loop"]
fn quack_query_surface_lifecycle_loop() {
    const ITERATIONS: u16 = 10;
    let mut failures = Vec::new();

    for iteration in 0..ITERATIONS {
        match run_one_iteration(iteration) {
            Ok(()) => println!("iteration {iteration}: PASS"),
            Err(e) => {
                println!("iteration {iteration}: FAIL — {e}");
                failures.push((iteration, e));
            }
        }
    }

    let passed = ITERATIONS as usize - failures.len();
    println!("\n=== quack query-surface stability loop: {passed}/{ITERATIONS} passed ===");
    for (iteration, error) in &failures {
        println!("  iteration {iteration} failed: {error}");
    }
    assert!(
        failures.is_empty(),
        "{}/{ITERATIONS} iterations failed: {failures:?}",
        failures.len()
    );
}
