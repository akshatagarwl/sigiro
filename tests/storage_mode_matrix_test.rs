//! End-to-end STORAGE MODE MATRIX: proves sigiro's dual-mode storage contract
//! works — ingest -> flush -> query -> RESTART-persistence — in each of the
//! three coherent points on the storage config axis:
//!
//!   1. LOCAL file catalog        (no CATALOG_URL, no S3): DuckDB `catalog.ducklake`
//!      file + local Parquet. Source of truth = the local data dir.
//!   2. PG catalog + LOCAL data   (CATALOG_URL, no S3_BUCKET): PostgreSQL metadata
//!      catalog, Parquet on local disk. Source of truth = PG catalog + local data dir.
//!   3. PG catalog + S3 data      (CATALOG_URL + S3_*): PostgreSQL metadata catalog,
//!      Parquet in RustFS/S3. Source of truth = PG + S3 (NOT the local disk).
//!
//! The persistence assertion is per-mode and encodes each mode's ACTUAL
//! durability contract:
//!   - modes 1 & 2 restart REUSING THE SAME data dir (local files are part of
//!     the source of truth — a fresh dir would legitimately lose the data);
//!   - mode 3 restarts with a BRAND-NEW data dir (PG + S3 are the whole source
//!     of truth), which is the stronger claim and mirrors
//!     `quack_s3_query_surface_test.rs`.
//!
//! All three modes enable the `quack` query surface over the pre-created
//! `default` project (the ONE ingest+read path that stays on static/open auth
//! even against a distributed catalog — see `auth::AuthMode::from_runtime_config`),
//! so the read side is identical across modes and the only variable is storage.
//!
//! The two PG-backed modes share one Postgres catalog + the same tenant
//! (`default`), so the shared metadata schema (`tenant_default`) is DROPPED
//! before each PG-backed mode to keep a stale DATA_PATH from a prior mode/run
//! from being reused (DuckLake bakes the DATA_PATH into the catalog on first
//! CREATE and ignores it thereafter).
//!
//! `#[ignore]`d — needs Docker (Postgres + RustFS). Run via
//! `mise run check-storage-matrix-e2e`, or directly:
//! ```text
//! docker compose -f docker-compose.dev.yml up -d --wait postgres rustfs
//! docker compose -f docker-compose.dev.yml up rustfs-setup
//! cargo test --test storage_mode_matrix_test -- --ignored --nocapture
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
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Matches `docker-compose.dev.yml` (see `quack_s3_query_surface_test.rs`).
const CATALOG_URL: &str = "postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake";
/// libpq form of the above, for the catalog-reset DuckDB->Postgres attach.
const CATALOG_LIBPQ: &str = "dbname=ducklake host=127.0.0.1 user=sigiro password=sigiro port=5432";
const S3_BUCKET: &str = "sigiro";
const S3_REGION: &str = "us-east-1";
const S3_ENDPOINT: &str = "http://127.0.0.1:9000";
const S3_KEY_ID: &str = "rustfsadmin";
const S3_SECRET: &str = "rustfsadmin";

/// The metadata schema DuckLake uses for the `default` tenant — dropped to
/// reset a PG-backed mode's catalog between modes/runs.
const DEFAULT_METADATA_SCHEMA: &str = "tenant_default";

/// Distinct from the port blocks the sibling quack E2Es use (41100+, 41300+)
/// so concurrently-running test binaries never collide on a listener.
const BASE_PORT: u16 = 41500;

const QUACK_TOKEN: &str = "storage-matrix-quack-token";

/// Shared DuckDB extension cache — see the sibling quack E2E's doc comment.
fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

/// One point on the storage config axis.
struct Mode {
    name: &'static str,
    /// Port offset from `BASE_PORT` so each mode's spawns don't collide.
    port_offset: u16,
    /// `Some` => distributed (PostgreSQL catalog); `None` => local file catalog.
    catalog_url: Option<&'static str>,
    /// Whether to configure S3/R2 data storage (implies `catalog_url` is set).
    s3: bool,
    /// Whether data survives a restart with a BRAND-NEW local data dir. True
    /// only when the source of truth is fully external (mode 3: PG + S3).
    persists_across_fresh_data_dir: bool,
}

const MODES: &[Mode] = &[
    Mode {
        name: "local file catalog",
        port_offset: 0,
        catalog_url: None,
        s3: false,
        persists_across_fresh_data_dir: false,
    },
    Mode {
        name: "PG catalog + local data",
        port_offset: 10,
        catalog_url: Some(CATALOG_URL),
        s3: false,
        persists_across_fresh_data_dir: false,
    },
    Mode {
        name: "PG catalog + S3 data",
        port_offset: 20,
        catalog_url: Some(CATALOG_URL),
        s3: true,
        persists_across_fresh_data_dir: true,
    },
];

/// A live `sigiro serve` child for one mode. Data dir is owned by the caller
/// (so it can be reused across a restart), not by this struct.
struct MatrixEdge {
    child: Child,
    _read_only_working_dir: tempfile::TempDir,
    otlp_http_port: u16,
    quack_port: u16,
}

impl MatrixEdge {
    fn spawn(mode: &Mode, data_dir: &Path) -> Self {
        let http_port = BASE_PORT + mode.port_offset;
        let otlp_http_port = BASE_PORT + mode.port_offset + 1;
        let grpc_port = BASE_PORT + mode.port_offset + 2;
        let quack_port = BASE_PORT + mode.port_offset + 3;
        let read_only_working_dir = tempfile::tempdir().expect("temporary working directory");
        let mut permissions = std::fs::metadata(read_only_working_dir.path())
            .expect("temporary working directory metadata")
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(read_only_working_dir.path(), permissions)
            .expect("make temporary working directory read-only");

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigiro"));
        cmd.arg("serve")
            .current_dir(read_only_working_dir.path())
            .env("SIGIRO_DATA_DIR", data_dir)
            .env("SIGIRO_HTTP_PORT", http_port.to_string())
            .env("SIGIRO_OTLP_HTTP_PORT", otlp_http_port.to_string())
            .env("SIGIRO_GRPC_PORT", grpc_port.to_string())
            .env("SIGIRO_QUERY_SURFACE", "quack")
            .env("SIGIRO_QUACK_BIND", "127.0.0.1")
            .env("SIGIRO_QUACK_PORT", quack_port.to_string())
            .env("SIGIRO_QUACK_TOKEN", QUACK_TOKEN)
            .env("SIGIRO_EXTENSION_DIR", test_extension_dir())
            // Pin the flush cadence: `wait_for_count` polls on a 20s deadline,
            // so the production 60s default would time it out.
            .env("SIGIRO_FLUSH_INTERVAL_SECS", "1")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(url) = mode.catalog_url {
            cmd.env("SIGIRO_CATALOG_URL", url);
        }
        if mode.s3 {
            cmd.env("SIGIRO_S3_BUCKET", S3_BUCKET)
                .env("SIGIRO_S3_REGION", S3_REGION)
                .env("SIGIRO_S3_ENDPOINT", S3_ENDPOINT)
                .env("SIGIRO_S3_KEY_ID", S3_KEY_ID)
                .env("SIGIRO_S3_SECRET", S3_SECRET);
        }

        let child = cmd.spawn().expect("failed to spawn `sigiro serve`");
        let edge = Self {
            child,
            _read_only_working_dir: read_only_working_dir,
            otlp_http_port,
            quack_port,
        };
        edge.wait_ready(http_port);
        edge
    }

    fn wait_ready(&self, http_port: u16) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Ok(resp) = reqwest::blocking::get(format!("http://127.0.0.1:{http_port}/health"))
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
        let tracer = provider.tracer("storage-matrix-e2e");
        for i in 0..count {
            let mut span = tracer.start(format!("op-{i}"));
            span.end();
        }
        provider.shutdown().expect("failed to flush OTLP spans");
    }

    fn quack_count(&self, service_name: &str) -> Result<i64, String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("LOAD quack;")
            .map_err(|e| e.to_string())?;
        let inner =
            format!("SELECT count(*) FROM lake.sigiro_spans WHERE service_name = '{service_name}'");
        let escaped = inner.replace('\'', "''");
        let wrapped = format!(
            "SELECT * FROM quack_query('quack://127.0.0.1:{}', '{escaped}', token := '{QUACK_TOKEN}', disable_ssl := true);",
            self.quack_port
        );
        conn.query_row(&wrapped, [], |row| row.get::<_, i64>(0))
            .map_err(|e| e.to_string())
    }

    fn wait_for_count(&self, service_name: &str, expected: i64) -> i64 {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let n = self.quack_count(service_name).unwrap_or(0);
            if n >= expected || Instant::now() >= deadline {
                return n;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

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
                    let mut output = String::new();
                    if let Some(mut stdout) = self.child.stdout.take() {
                        stdout
                            .read_to_string(&mut output)
                            .map_err(|e| format!("read `sigiro serve` stdout: {e}"))?;
                    }
                    if let Some(mut stderr) = self.child.stderr.take() {
                        stderr
                            .read_to_string(&mut output)
                            .map_err(|e| format!("read `sigiro serve` stderr: {e}"))?;
                    }
                    if !status.success() {
                        return Err(format!(
                            "`sigiro serve` exited non-zero: {status}\n{output}"
                        ));
                    }
                    if output.contains("CHECKPOINT failed") {
                        return Err(format!("`sigiro serve` checkpoint failed:\n{output}"));
                    }
                    if !output.contains("DuckDB temp directory configured") {
                        return Err(format!(
                            "`sigiro serve` did not configure its temp directory:\n{output}"
                        ));
                    }
                    return Ok(());
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

/// Drop the DuckLake metadata schema for the `default` tenant from the shared
/// Postgres catalog, so a PG-backed mode starts from a clean catalog and its
/// DATA_PATH (local vs S3) is the one that actually takes effect. Uses DuckDB's
/// `postgres` extension exactly as the production distributed-storage path does.
fn reset_pg_catalog() {
    let conn =
        duckdb::Connection::open_in_memory().expect("open in-memory DuckDB for catalog reset");
    conn.execute_batch(&format!(
        "SET extension_directory = '{}';",
        test_extension_dir().replace('\'', "''")
    ))
    .expect("set extension_directory");
    conn.execute_batch("INSTALL postgres; LOAD postgres;")
        .expect("load postgres extension");
    conn.execute_batch(&format!(
        "ATTACH '{CATALOG_LIBPQ}' AS pgcat (TYPE postgres)"
    ))
    .expect("attach Postgres catalog for reset");
    conn.execute_batch(&format!(
        "CALL postgres_execute('pgcat', 'DROP SCHEMA IF EXISTS {DEFAULT_METADATA_SCHEMA} CASCADE')"
    ))
    .expect("drop tenant_default schema");
}

fn run_mode(mode: &Mode) -> Result<(), String> {
    const SPANS: usize = 10;
    let service_name = format!("matrix-{}-{}", mode.port_offset, std::process::id());

    if mode.catalog_url.is_some() {
        reset_pg_catalog();
    }

    // First boot: ingest, flush, read back.
    let dir1 = tempfile::tempdir().expect("temp data dir");
    let edge = MatrixEdge::spawn(mode, dir1.path());
    edge.ingest_spans(&service_name, SPANS);
    let seen = edge.wait_for_count(&service_name, SPANS as i64);
    if seen != SPANS as i64 {
        if let Err(e) = edge.shutdown() {
            println!("  (cleanup: shutdown after count mismatch failed: {e})");
        }
        return Err(format!(
            "[{}] expected {SPANS} spans visible via quack after ingest, saw {seen}",
            mode.name
        ));
    }
    edge.shutdown()
        .map_err(|e| format!("[{}] first-boot shutdown: {e}", mode.name))?;
    println!(
        "  [{}] ingest + flush + quack read OK ({seen} spans)",
        mode.name
    );

    // Restart. Per-mode: fresh data dir only when the source of truth is fully
    // external (mode 3). Otherwise reuse the same data dir.
    let dir2;
    let restart_dir = if mode.persists_across_fresh_data_dir {
        dir2 = tempfile::tempdir().expect("fresh temp data dir");
        dir2.path()
    } else {
        dir1.path()
    };
    let edge2 = MatrixEdge::spawn(mode, restart_dir);
    let after_restart = edge2
        .quack_count(&service_name)
        .map_err(|e| format!("[{}] post-restart query: {e}", mode.name))?;
    let restart_result = if after_restart != SPANS as i64 {
        Err(format!(
            "[{}] data did not survive restart ({}): expected {SPANS}, got {after_restart}",
            mode.name,
            if mode.persists_across_fresh_data_dir {
                "fresh data dir"
            } else {
                "same data dir"
            }
        ))
    } else {
        println!(
            "  [{}] persistence OK — {after_restart} spans survived restart ({})",
            mode.name,
            if mode.persists_across_fresh_data_dir {
                "fresh data dir; PG + S3 are the source of truth"
            } else {
                "same data dir"
            }
        );
        Ok(())
    };
    edge2
        .shutdown()
        .map_err(|e| format!("[{}] restart shutdown: {e}", mode.name))?;
    restart_result
}

#[test]
#[ignore = "needs Docker (Postgres catalog + RustFS S3) — run via `mise run check-storage-matrix-e2e`"]
fn storage_mode_matrix() {
    for mode in MODES {
        println!("== storage mode: {} ==", mode.name);
        run_mode(mode).unwrap_or_else(|e| panic!("storage mode matrix failed: {e}"));
    }
    println!("all three storage modes passed: ingest -> flush -> query -> restart-persistence");
}
