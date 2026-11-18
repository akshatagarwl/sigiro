#![allow(clippy::disallowed_methods)]
// The harness reads and writes the process environment directly to drive the
// CLI exactly as an operator would. `clippy.toml` bans that in production code
// so every reader goes through typed-env.

//! End-to-end contract for the batch run ledger.
//!
//! Runs the real `sigiro` binary against a real DuckLake catalog: writes a
//! Parquet kernel export and a run-record JSON to disk, ingests them through
//! `sigiro ingest-kernels` / `sigiro record-run`, then reads the catalog back
//! the way any query would. Nothing is stubbed — the failure cases assert the
//! process exits non-zero AND leaves the ledger untouched.

use std::path::Path;
use std::process::{Command, Output};

use duckdb::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;

fn extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

/// Run the real CLI against a data directory the test owns.
fn sigiro(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sigiro"))
        .args(args)
        .env("SIGIRO_DATA_DIR", data_dir)
        .env("SIGIRO_EXTENSION_DIR", extension_dir())
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "")
        .env("RUST_LOG", "warn")
        .output()
        .expect("failed to run the sigiro binary")
}

fn run_ok(data_dir: &Path, args: &[&str]) -> Value {
    let output = sigiro(data_dir, args);
    assert!(
        output.status.success(),
        "`sigiro {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI must print JSON on success")
}

/// Assert the command fails and its error names the actual cause.
fn run_err(data_dir: &Path, args: &[&str], expected: &str) {
    let output = sigiro(data_dir, args);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "`sigiro {}` should have failed; stdout: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains(expected),
        "`sigiro {}` error should mention {expected:?}, got: {stderr}",
        args.join(" ")
    );
}

/// Write a Parquet file from a SELECT, the way a profiler's exporter would.
fn write_parquet(path: &Path, select: &str) {
    let conn = Connection::open_in_memory().expect("in-memory DuckDB");
    conn.execute_batch(&format!(
        "COPY ({select}) TO '{}' (FORMAT PARQUET)",
        path.display()
    ))
    .expect("write Parquet fixture");
}

/// Three aggregated kernel rows for one run: two attributed, one residual.
/// `extra` is appended to every row's column list so a variant can drop a
/// column without duplicating the whole projection.
fn kernel_rows(run_id: &str) -> String {
    let row = |kernel: &str, bucket: &str, layer: &str, total_ns: i64, executions: i64| {
        format!(
            "SELECT '{run_id}' AS run_id, 'exp-ledger-1' AS experiment_id, 'candidate' AS arm, \
             TIMESTAMP '2026-08-16 12:00:00' AS timestamp, 'vllm' AS service_name, \
             'cuda:0' AS device, 0::INTEGER AS rank, 7::BIGINT AS graph_id, \
             {executions}::BIGINT AS node_index, '{kernel}' AS kernel, '{layer}' AS layer, \
             'matmul' AS op, 'graph-node' AS attribution_method, '{bucket}' AS bucket, \
             {executions}::BIGINT AS executions, {total_ns}::BIGINT AS total_ns, \
             ({total_ns} / {executions})::BIGINT AS mean_ns, {total_ns}::BIGINT AS max_ns, \
             62.5 AS occupancy_pct, 128::INTEGER AS registers_per_thread, \
             49152::BIGINT AS shared_mem_bytes, 'memory' AS limiting_factor, \
             'node_1,node_2' AS source_nodes"
        )
    };
    format!(
        "{} UNION ALL {} UNION ALL {}",
        row("sm90_gemm", "attributed", "model.layers.0.mlp", 4_000, 4),
        row(
            "flash_attn_fwd",
            "attributed",
            "model.layers.0.attn",
            6_000,
            2
        ),
        row("unknown_kernel", "residual_no_provenance", "", 1_000, 1),
    )
}

fn run_record(run_id: &str, parent_run_id: Option<&str>) -> Value {
    json!({
        "experiment_id": "exp-ledger-1",
        "run_id": run_id,
        "arm": "candidate",
        "parent_run_id": parent_run_id,
        "started_at": 1_780_000_000_000_000i64,
        "ended_at": 1_780_000_300_000_000i64,
        "config_sha": "a".repeat(64),
        "engine": "vllm",
        "engine_version": "0.11.0",
        "model_id": "meta-llama/Llama-3.1-8B",
        "gpu_model": "H100",
        "gpu_count": 8,
        "tp_size": 8,
        "quantization": "fp8",
        "config_json": { "max_num_seqs": 256, "enable_chunked_prefill": true },
        "goodput": 1234.5,
        "ttft_p95_ms": 180.25,
        "gpu_seconds": 2400.0,
        "price_per_hour_usd": 29.5,
        "outcome": "confirmed",
        "drift_detected": false
    })
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).expect("serialize")).expect("write JSON");
}

/// Read the ledger back the way a query would — the CLI process has exited by
/// the time this attaches, so this sees only what was committed to Parquet.
fn open_ledger(data_dir: &Path) -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory DuckDB");
    conn.execute_batch(&format!("SET extension_directory='{}';", extension_dir()))
        .expect("set extension_directory");
    conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
        .expect("install/load ducklake");
    conn.execute_batch(&format!(
        "ATTACH 'ducklake:{}/default/catalog.ducklake' AS lake",
        data_dir.display()
    ))
    .expect("attach the ledger catalog written by the CLI");
    conn
}

fn scalar<T: duckdb::types::FromSql>(conn: &Connection, sql: &str) -> T {
    conn.query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("query failed: {sql}: {e}"))
}

#[test]
fn kernel_export_and_run_record_land_in_the_ledger() {
    let data_dir = TempDir::new().expect("data directory");
    let work = TempDir::new().expect("working directory");
    let parquet = work.path().join("kernels.parquet");
    write_parquet(&parquet, &kernel_rows("run-002"));
    let record = work.path().join("run.json");
    write_json(&record, &run_record("run-002", Some("run-001")));

    let ingested = run_ok(
        data_dir.path(),
        &["ingest-kernels", "--file", parquet.to_str().unwrap()],
    );
    assert_eq!(ingested["rows_inserted"], json!(3));
    assert_eq!(ingested["table"], json!("sigiro_gpu_kernels"));

    let recorded = run_ok(
        data_dir.path(),
        &["record-run", "--file", record.to_str().unwrap()],
    );
    assert_eq!(recorded["run_id"], json!("run-002"));

    let conn = open_ledger(data_dir.path());

    // Every nanosecond of the run is present, residual included — that is the
    // whole point of a non-null bucket.
    let total_ns: i64 = scalar(
        &conn,
        "SELECT sum(total_ns) FROM lake.sigiro_gpu_kernels WHERE run_id = 'run-002'",
    );
    assert_eq!(total_ns, 11_000);
    let residual_ns: i64 = scalar(
        &conn,
        "SELECT sum(total_ns) FROM lake.sigiro_gpu_kernels \
         WHERE bucket = 'residual_no_provenance'",
    );
    assert_eq!(residual_ns, 1_000);
    let null_buckets: i64 = scalar(
        &conn,
        "SELECT count(*) FROM lake.sigiro_gpu_kernels WHERE bucket IS NULL",
    );
    assert_eq!(null_buckets, 0);
    let occupancy: f64 = scalar(
        &conn,
        "SELECT max(occupancy_pct) FROM lake.sigiro_gpu_kernels WHERE run_id = 'run-002'",
    );
    assert!((occupancy - 62.5).abs() < f64::EPSILON);

    // The run row keeps its identity, its results, and its lineage edge.
    let started_at: String = scalar(
        &conn,
        "SELECT CAST(started_at AS VARCHAR) FROM lake.sigiro_experiment_runs \
         WHERE run_id = 'run-002'",
    );
    assert_eq!(started_at, "2026-05-28 20:26:40");
    let parent: String = scalar(
        &conn,
        "SELECT parent_run_id FROM lake.sigiro_experiment_runs WHERE run_id = 'run-002'",
    );
    assert_eq!(parent, "run-001");
    let config_json: String = scalar(
        &conn,
        "SELECT config_json FROM lake.sigiro_experiment_runs WHERE run_id = 'run-002'",
    );
    assert!(
        config_json.contains("\"max_num_seqs\":256"),
        "config_json must keep the runtime configuration verbatim, got: {config_json}"
    );
    let goodput: f64 = scalar(
        &conn,
        "SELECT goodput FROM lake.sigiro_experiment_runs WHERE run_id = 'run-002'",
    );
    assert!((goodput - 1234.5).abs() < f64::EPSILON);
    let drift: bool = scalar(
        &conn,
        "SELECT drift_detected FROM lake.sigiro_experiment_runs WHERE run_id = 'run-002'",
    );
    assert!(!drift);
    drop(conn);

    // Re-running either command would double the run's GPU time and its
    // verdict, so both refuse instead of appending a second copy.
    run_err(
        data_dir.path(),
        &["ingest-kernels", "--file", parquet.to_str().unwrap()],
        "already has rows for run(s) run-002",
    );
    run_err(
        data_dir.path(),
        &["record-run", "--file", record.to_str().unwrap()],
        "already has run run-002",
    );

    let conn = open_ledger(data_dir.path());
    let kernels: i64 = scalar(&conn, "SELECT count(*) FROM lake.sigiro_gpu_kernels");
    assert_eq!(kernels, 3, "a refused re-ingest must not append rows");
    let runs: i64 = scalar(&conn, "SELECT count(*) FROM lake.sigiro_experiment_runs");
    assert_eq!(runs, 1, "a refused re-record must not append rows");
    drop(conn);

    // The verdict is only known once every period has run, so it arrives as its
    // own command and lands on every row of the experiment — the point being
    // that "did it help" is answerable from this one table with no join.
    let finalized = run_ok(
        data_dir.path(),
        &[
            "finalize-experiment",
            "--experiment-id",
            "exp-ledger-1",
            "--outcome",
            "drift_detected",
        ],
    );
    assert_eq!(finalized["rows_updated"], json!(1));
    assert_eq!(finalized["outcome"], json!("drift_detected"));

    let conn = open_ledger(data_dir.path());
    let outcome: String = scalar(
        &conn,
        "SELECT outcome FROM lake.sigiro_experiment_runs WHERE run_id = 'run-002'",
    );
    assert_eq!(outcome, "drift_detected");
    // Derived from the outcome, never passed alongside it, so the two can never
    // disagree on the same row.
    let drift: bool = scalar(
        &conn,
        "SELECT drift_detected FROM lake.sigiro_experiment_runs WHERE run_id = 'run-002'",
    );
    assert!(drift);
}

#[test]
fn malformed_input_is_refused_and_writes_nothing() {
    let data_dir = TempDir::new().expect("data directory");
    let work = TempDir::new().expect("working directory");

    // A null bucket means unattributed GPU time lost its reason.
    let null_bucket = work.path().join("null-bucket.parquet");
    write_parquet(
        &null_bucket,
        &kernel_rows("run-003").replace("'residual_no_provenance' AS bucket", "NULL AS bucket"),
    );
    run_err(
        data_dir.path(),
        &["ingest-kernels", "--file", null_bucket.to_str().unwrap()],
        "row(s) with a null bucket",
    );

    // A missing column must name itself rather than shift every value left.
    let missing_column = work.path().join("missing-column.parquet");
    write_parquet(
        &missing_column,
        &format!(
            "SELECT * EXCLUDE (total_ns) FROM ({})",
            kernel_rows("run-004")
        ),
    );
    run_err(
        data_dir.path(),
        &["ingest-kernels", "--file", missing_column.to_str().unwrap()],
        "must have every column of the table",
    );

    run_err(
        data_dir.path(),
        &["ingest-kernels", "--file", "/nonexistent/kernels.parquet"],
        "kernel Parquet file not found",
    );

    // A run record without `arm` cannot be placed on either side of a
    // comparison, so it is a parse failure that names the field.
    let mut incomplete = run_record("run-005", None);
    incomplete
        .as_object_mut()
        .expect("object")
        .remove("arm")
        .expect("arm was set");
    let incomplete_path = work.path().join("incomplete.json");
    write_json(&incomplete_path, &incomplete);
    run_err(
        data_dir.path(),
        &["record-run", "--file", incomplete_path.to_str().unwrap()],
        "missing field `arm`",
    );

    // A typo'd field must fail rather than silently leave a column null.
    let mut typo = run_record("run-006", None);
    let object = typo.as_object_mut().expect("object");
    object.remove("goodput");
    object.insert("good_put".to_string(), json!(1.0));
    let typo_path = work.path().join("typo.json");
    write_json(&typo_path, &typo);
    run_err(
        data_dir.path(),
        &["record-run", "--file", typo_path.to_str().unwrap()],
        "good_put",
    );

    // Seconds where microseconds belong would silently record a 1970 run.
    let mut wrong_unit = run_record("run-007", None);
    wrong_unit
        .as_object_mut()
        .expect("object")
        .insert("started_at".to_string(), json!(1_780_000_000i64));
    let wrong_unit_path = work.path().join("wrong-unit.json");
    write_json(&wrong_unit_path, &wrong_unit);
    run_err(
        data_dir.path(),
        &["record-run", "--file", wrong_unit_path.to_str().unwrap()],
        "not a plausible 16-digit microsecond timestamp",
    );

    // A verdict outside the vocabulary /v1/experiments/compare produces would be
    // unreadable to every query that filters on `outcome`.
    run_err(
        data_dir.path(),
        &[
            "finalize-experiment",
            "--experiment-id",
            "exp-ledger-1",
            "--outcome",
            "faster",
        ],
        "unknown variant `faster`",
    );

    // Finalizing an experiment with no rows writes the verdict nowhere, so it
    // must fail rather than report success.
    run_err(
        data_dir.path(),
        &[
            "finalize-experiment",
            "--experiment-id",
            "exp-never-recorded",
            "--outcome",
            "confirmed",
        ],
        "has no rows for experiment exp-never-recorded",
    );

    let conn = open_ledger(data_dir.path());
    let kernels: i64 = scalar(&conn, "SELECT count(*) FROM lake.sigiro_gpu_kernels");
    assert_eq!(kernels, 0, "no refused kernel export may leave rows behind");
    let runs: i64 = scalar(&conn, "SELECT count(*) FROM lake.sigiro_experiment_runs");
    assert_eq!(runs, 0, "no refused run record may leave rows behind");
}
