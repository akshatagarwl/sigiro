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
//! The two PG-backed modes speak TLS, because production does: `storage.rs`'s
//! `validate_hosted_catalog_tls` refuses a hosted catalog URL that is not
//! `sslmode=verify-full` with a non-empty `sslrootcert`. `scripts/dev-certs.sh`
//! issues the dev Postgres an mkcert-signed server certificate and writes the
//! matching CA to `.dev/certs/ca.crt`, so `verify-full` here GENUINELY verifies
//! rather than being downgraded to `require` — the local run exercises the same
//! code path production does. (Confirmed: pointing `sslrootcert` at an
//! unrelated root fails with "SSL error: certificate verify failed".)
//!
//! `#[ignore]`d — needs the dev stack (Postgres + TLS + RustFS). Run via
//! `mise run check-storage-matrix-e2e`, which issues the certs, brings the
//! stack up, and tears it down.

#![allow(clippy::disallowed_methods)]
// Test harnesses read and write the process environment directly to drive
// storage/config permutations. `clippy.toml` bans that in production code so
// every reader goes through typed-env; a test-only knob routed through
// typed-env would add a production config entry production never reads.

use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::resource::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// CA that signed the dev stack's PostgreSQL server certificate, written by
/// `scripts/dev-certs.sh` into the repository's (gitignored) `.dev/certs/`.
/// `sslmode=verify-full` is checked against THIS root, so the local run
/// verifies for real instead of trusting anything presented.
fn catalog_ca_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("SIGIRO_DEV_CATALOG_CA")
        && !explicit.is_empty()
    {
        return PathBuf::from(explicit);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".dev")
        .join("certs")
        .join("ca.crt")
}

/// Matches `www/alchemy.dev.run.ts` (see `quack_s3_query_surface_test.rs`).
/// `sslmode=verify-full` + `sslrootcert` are NOT decoration: `storage.rs`'s
/// `validate_hosted_catalog_tls` refuses any other hosted catalog URL, so a
/// plain `postgresql://…` here makes both PG modes fail to boot at all.
fn catalog_url() -> String {
    format!(
        "postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake?sslmode=verify-full&sslrootcert={}",
        catalog_ca_path().display()
    )
}

/// libpq form of the above, for the catalog-reset DuckDB->Postgres attach.
fn catalog_libpq() -> String {
    format!(
        "dbname=ducklake host=127.0.0.1 user=sigiro password=sigiro port=5432 sslmode=verify-full sslrootcert={}",
        catalog_ca_path().display()
    )
}

const S3_BUCKET: &str = "sigiro";
const S3_REGION: &str = "us-east-1";
const S3_ENDPOINT: &str = "http://127.0.0.1:9000";
const S3_KEY_ID: &str = "rustfsadmin";
const S3_SECRET: &str = "rustfsadmin";

/// Hosted (PostgreSQL-catalog) mode has a filesystem contract production owns:
/// `/var/lib/sigiro/telemetry` and `/var/cache/sigiro/duckdb`. Neither is
/// writable on a developer machine, and `run_serve` switches to
/// `hosted_data_dir()` the moment `SIGIRO_CATALOG_URL` is set — so the test's
/// own temp dir is only honoured when these are pointed at it too.
const HOSTED_DATA_SUBDIR: &str = "hosted";
const DUCKDB_CACHE_SUBDIR: &str = "duckdb-cache";

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
    /// `true` => distributed (PostgreSQL catalog); `false` => local file catalog.
    distributed: bool,
    /// Whether to configure S3/R2 data storage (implies `distributed`).
    s3: bool,
    /// Whether data survives a restart with a BRAND-NEW local data dir. True
    /// only when the source of truth is fully external (mode 3: PG + S3).
    persists_across_fresh_data_dir: bool,
}

const MODES: &[Mode] = &[
    Mode {
        name: "local file catalog",
        port_offset: 0,
        distributed: false,
        s3: false,
        persists_across_fresh_data_dir: false,
    },
    Mode {
        name: "PG catalog + local data",
        port_offset: 10,
        distributed: true,
        s3: false,
        persists_across_fresh_data_dir: false,
    },
    Mode {
        name: "PG catalog + S3 data",
        port_offset: 20,
        distributed: true,
        s3: true,
        persists_across_fresh_data_dir: true,
    },
];

/// Everything the child wrote to stdout+stderr, drained concurrently by two
/// reader threads.
///
/// Draining has to happen WHILE the child runs, not after it exits. The old
/// harness read the pipes only in `shutdown()`, so a child that failed during
/// startup — the single most common failure, and the one config mistakes
/// produce — was reported as a bare 90-second `/health` timeout with its
/// actual `Error: …` line still sitting unread in the pipe. A 64KiB pipe also
/// blocks a chatty child forever once it fills.
#[derive(Clone, Default)]
struct ChildLog(Arc<Mutex<String>>);

impl ChildLog {
    fn drain(&self, stream: impl std::io::Read + Send + 'static) -> JoinHandle<()> {
        let sink = Arc::clone(&self.0);
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let mut buffer = sink.lock().expect("child log mutex");
                buffer.push_str(&line);
                buffer.push('\n');
            }
        })
    }

    fn snapshot(&self) -> String {
        self.0.lock().expect("child log mutex").clone()
    }
}

/// Render a child's captured output for a failure message, or say plainly that
/// it produced none (which is itself the diagnosis: the process never spoke).
fn describe_output(log: &ChildLog) -> String {
    let captured = log.snapshot();
    if captured.trim().is_empty() {
        "  (`sigiro serve` produced no stdout/stderr at all)".to_string()
    } else {
        captured
            .lines()
            .map(|line| format!("  | {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A live `sigiro serve` child for one mode. Data dir is owned by the caller
/// (so it can be reused across a restart), not by this struct.
struct MatrixEdge {
    child: Child,
    log: ChildLog,
    readers: Vec<JoinHandle<()>>,
    _read_only_working_dir: tempfile::TempDir,
    otlp_http_port: u16,
    quack_port: u16,
}

impl MatrixEdge {
    fn spawn(mode: &Mode, data_dir: &Path) -> Result<Self, String> {
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

        if mode.distributed {
            // Hosted mode ignores SIGIRO_DATA_DIR and uses these two instead
            // (see `run_serve` and `resolve_extension_dir`). Their production
            // defaults — /var/lib/sigiro/telemetry and /var/cache/sigiro/duckdb
            // — do not exist on a developer machine.
            let hosted = data_dir.join(HOSTED_DATA_SUBDIR);
            let cache = data_dir.join(DUCKDB_CACHE_SUBDIR);
            std::fs::create_dir_all(&hosted)
                .map_err(|e| format!("create hosted data dir {}: {e}", hosted.display()))?;
            std::fs::create_dir_all(&cache)
                .map_err(|e| format!("create DuckDB cache dir {}: {e}", cache.display()))?;
            cmd.env("SIGIRO_CATALOG_URL", catalog_url())
                .env("SIGIRO_HOSTED_DATA_DIR", &hosted)
                .env("SIGIRO_DUCKDB_CACHE_DIR", &cache);
        }
        if mode.s3 {
            cmd.env("SIGIRO_S3_BUCKET", S3_BUCKET)
                .env("SIGIRO_S3_REGION", S3_REGION)
                .env("SIGIRO_S3_ENDPOINT", S3_ENDPOINT)
                .env("SIGIRO_S3_KEY_ID", S3_KEY_ID)
                .env("SIGIRO_S3_SECRET", S3_SECRET);
        }

        let mut child = cmd.spawn().map_err(|e| format!("spawn `sigiro serve`: {e}"))?;
        let log = ChildLog::default();
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(log.drain(stdout));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(log.drain(stderr));
        }
        let edge = Self {
            child,
            log,
            readers,
            _read_only_working_dir: read_only_working_dir,
            otlp_http_port,
            quack_port,
        };
        edge.wait_ready(http_port)?;
        Ok(edge)
    }

    /// Poll `/health` until it reports ready, the child dies, or 90s elapse.
    ///
    /// Every failure path quotes what the child actually said. An early exit
    /// is reported the instant it happens rather than after the full deadline:
    /// a process that is already gone will never become ready.
    fn wait_ready(&self, http_port: u16) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if let Ok(resp) = reqwest::blocking::get(format!("http://127.0.0.1:{http_port}/health"))
                && resp.status().is_success()
                && let Ok(body) = resp.json::<serde_json::Value>()
                && body.get("status").and_then(|v| v.as_str()) == Some("ready")
            {
                return Ok(());
            }
            // SAFETY: waitpid(WNOHANG) on our own child. `try_wait` needs
            // &mut self, which the poll loop does not hold.
            let exited = unsafe {
                let mut status = 0;
                libc::waitpid(self.child.id() as libc::pid_t, &mut status, libc::WNOHANG) > 0
            };
            if exited {
                // Give the reader threads a moment to flush the final lines.
                std::thread::sleep(Duration::from_millis(200));
                return Err(format!(
                    "`sigiro serve` exited before reporting /health ready. Its output:\n{}",
                    describe_output(&self.log)
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "`sigiro serve` did not report /health ready within 90s. Its output:\n{}",
                    describe_output(&self.log)
                ));
            }
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

    /// Join both reader threads, which finish once the child closes its pipes,
    /// and return everything it ever wrote. Must be called AFTER the child has
    /// exited, otherwise the joins block until it does.
    fn drain_log(&mut self) -> String {
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        self.log.snapshot()
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
                    // The pipes were moved into the reader threads at spawn, so
                    // the log lives in `self.log` — reading `child.stdout` here
                    // (as this harness used to) would see an empty string and
                    // turn every assertion below into a lie.
                    let output = self.drain_log();
                    if !status.success() {
                        return Err(format!(
                            "`sigiro serve` exited non-zero: {status}\n{}",
                            describe_output(&self.log)
                        ));
                    }
                    if output.contains("CHECKPOINT failed") {
                        return Err(format!(
                            "`sigiro serve` checkpoint failed:\n{}",
                            describe_output(&self.log)
                        ));
                    }
                    if !output.contains("DuckDB temp directory configured") {
                        return Err(format!(
                            "`sigiro serve` did not configure its temp directory:\n{}",
                            describe_output(&self.log)
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
                        let _ = self.drain_log();
                        return Err(format!(
                            "`sigiro serve` did not exit within 15s of SIGTERM. Its output:\n{}",
                            describe_output(&self.log)
                        ));
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
fn reset_pg_catalog() -> Result<(), String> {
    let conn = duckdb::Connection::open_in_memory()
        .map_err(|e| format!("open in-memory DuckDB for catalog reset: {e}"))?;
    conn.execute_batch(&format!(
        "SET extension_directory = '{}';",
        test_extension_dir().replace('\'', "''")
    ))
    .map_err(|e| format!("set extension_directory: {e}"))?;
    conn.execute_batch("INSTALL postgres; LOAD postgres;")
        .map_err(|e| format!("load DuckDB postgres extension: {e}"))?;
    // `sslmode=verify-full` + `sslrootcert` go through here too. DuckDB's
    // postgres extension passes the keyword string straight to libpq, so this
    // attach genuinely verifies the dev stack's mkcert-issued certificate
    // against the dev CA — a mismatched root fails with
    // "SSL error: certificate verify failed" rather than silently downgrading.
    let libpq = catalog_libpq();
    conn.execute_batch(&format!("ATTACH '{libpq}' AS pgcat (TYPE postgres)"))
        .map_err(|e| {
            format!(
                "attach Postgres catalog for reset (is the dev stack up, and is {} the CA that \
                 signed its server certificate?): {e}",
                catalog_ca_path().display()
            )
        })?;
    conn.execute_batch(&format!(
        "CALL postgres_execute('pgcat', 'DROP SCHEMA IF EXISTS {DEFAULT_METADATA_SCHEMA} CASCADE')"
    ))
    .map_err(|e| format!("drop {DEFAULT_METADATA_SCHEMA} schema: {e}"))
}

fn run_mode(mode: &Mode) -> Result<(), String> {
    const SPANS: usize = 10;
    let service_name = format!("matrix-{}-{}", mode.port_offset, std::process::id());

    if mode.distributed {
        reset_pg_catalog().map_err(|e| format!("[{}] catalog reset: {e}", mode.name))?;
    }

    // First boot: ingest, flush, read back.
    let dir1 = tempfile::tempdir().expect("temp data dir");
    let edge = MatrixEdge::spawn(mode, dir1.path())
        .map_err(|e| format!("[{}] first boot: {e}", mode.name))?;
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
    let edge2 = MatrixEdge::spawn(mode, restart_dir)
        .map_err(|e| format!("[{}] restart boot: {e}", mode.name))?;
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
