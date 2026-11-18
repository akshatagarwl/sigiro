//! Batch, run-scoped ingest for the two run-ledger tables.
//!
//! `sigiro ingest-kernels` loads a profiler's Parquet export into
//! `sigiro_gpu_kernels`; `sigiro record-run` records one finished run into
//! `sigiro_experiment_runs`; `sigiro finalize-experiment` writes the verdict
//! onto that experiment's rows once the comparison has produced one. All three
//! are deliberately file- or id-shaped CLI commands and NOT OTLP or HTTP
//! routes: kernel timing and run identity exist only once a run has ended,
//! they arrive as whatever the producer wrote to disk, and they are written by
//! the same operator who owns the data directory.
//!
//! Both ingest commands refuse to write a run that is already in the ledger.
//! These tables are aggregates (one row per kernel node, one row per run), so a
//! re-run of the same file would double the GPU time and the verdict instead
//! of failing — the silent corruption this module exists to prevent.

use anyhow::{Context, Result, bail};
use camino::Utf8Path;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::experiment::{ExperimentOutcome, MICROS_MAX, MICROS_MIN};
use crate::pool::ProjectPool;
use crate::sql::escape_sql_string;

/// Every column of `sigiro_gpu_kernels`, in table order.
///
/// Named explicitly on both sides of the `INSERT ... SELECT` so a Parquet
/// file that is missing one fails at bind time — DuckDB names the missing
/// column — instead of positionally shifting every value one column left.
const KERNEL_COLUMNS: [&str; 23] = [
    "run_id",
    "experiment_id",
    "arm",
    "timestamp",
    "service_name",
    "device",
    "rank",
    "graph_id",
    "node_index",
    "kernel",
    "layer",
    "op",
    "attribution_method",
    "bucket",
    "executions",
    "total_ns",
    "mean_ns",
    "max_ns",
    "occupancy_pct",
    "registers_per_thread",
    "shared_mem_bytes",
    "limiting_factor",
    "source_nodes",
];

/// One finished run, exactly as `sigiro record-run` reads it from JSON.
///
/// Required fields are plain (a missing one is a serde parse failure naming
/// it); everything else is `Option` and lands as SQL NULL. Unknown fields are
/// rejected, so a typo in a hand-written run record is an error rather than a
/// column that silently stays null.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRunRecord {
    pub experiment_id: String,
    pub run_id: String,
    /// Which side of the experiment this run is, e.g. "control"/"candidate".
    pub arm: String,
    /// The run this one was derived from — the lineage edge.
    pub parent_run_id: Option<String>,
    /// Microseconds since the Unix epoch, matching every other timestamp in
    /// the API surface (see `experiment::ExperimentPeriod`).
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub config_sha: Option<String>,
    pub model_sha: Option<String>,
    pub hardware_sha: Option<String>,
    pub benchmark_sha: Option<String>,
    pub engine: Option<String>,
    pub engine_version: Option<String>,
    pub model_id: Option<String>,
    pub model_revision: Option<String>,
    pub gpu_model: Option<String>,
    pub gpu_count: Option<i32>,
    pub tp_size: Option<i32>,
    pub quantization: Option<String>,
    /// The full runtime configuration. Must be a JSON object — stored as its
    /// compact JSON text, so a scalar here is a parse error rather than a
    /// string that only looks like a config.
    pub config_json: Option<serde_json::Map<String, serde_json::Value>>,
    pub goodput: Option<f64>,
    pub ttft_p95_ms: Option<f64>,
    pub itl_ms: Option<f64>,
    pub output_tokens_per_s: Option<f64>,
    pub gpu_seconds: Option<f64>,
    pub price_per_hour_usd: Option<f64>,
    pub outcome: Option<String>,
    pub drift_detected: Option<bool>,
}

/// Open the DuckLake catalog of a project directly, without the connection pool.
///
/// The pool is the wrong tool here for one specific reason: it ends every
/// local-mode setup with `enable_external_access = false`
/// (`ProjectPool::lock_filesystem`), which is instance-global, irreversible,
/// and blocks `read_parquet` on any path outside the data directory. These
/// commands exist to read a file the operator names on the command line, so
/// they must keep filesystem access. That is safe here and not in the server:
/// this is a single-shot CLI process run by the owner of the data directory,
/// not a request handler serving an untrusted tenant's SQL.
pub fn open_ledger_db(data_dir: &Utf8Path, project_id: &str) -> Result<Connection> {
    let conn = Connection::open_in_memory().context("failed to open in-memory DuckDB")?;
    let extension_dir = crate::storage::resolve_extension_dir(data_dir);
    conn.execute_batch(&format!(
        "SET extension_directory = '{}';",
        escape_sql_string(extension_dir.as_str())
    ))
    .context("failed to set DuckDB extension directory")?;

    let storage_mode = crate::storage::StorageMode::from_env()?;
    let project_dir = if storage_mode.is_distributed() {
        crate::storage::tenant_data_dir(data_dir, project_id)?
    } else {
        data_dir.join(project_id)
    };
    match storage_mode.distributed_config() {
        Some(config) => crate::storage::init_distributed(&conn, config, &project_dir, project_id),
        None => crate::storage::init_default(&conn, &project_dir),
    }
    .with_context(|| format!("failed to attach the DuckLake catalog for project {project_id}"))?;
    crate::schema::create_tables(&conn, ProjectPool::prefix())?;
    Ok(conn)
}

/// Load an aggregated GPU-kernel Parquet export into `sigiro_gpu_kernels`.
///
/// Returns the number of rows inserted.
pub fn ingest_kernels(conn: &Connection, file: &Utf8Path) -> Result<usize> {
    if !file.is_file() {
        bail!("kernel Parquet file not found: {file}");
    }
    let prefix = ProjectPool::prefix();
    let columns = KERNEL_COLUMNS.join(", ");
    let source = format!("read_parquet('{}')", escape_sql_string(file.as_str()));

    // Unattributed GPU time must be a row carrying its residual reason, never
    // a null bucket — a null here means the producer dropped the reason and
    // every share-of-total computed from this file would be unexplainable.
    let null_buckets: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {source} WHERE bucket IS NULL"),
            [],
            |row| row.get(0),
        )
        .with_context(|| format!("failed to read kernel Parquet file {file}"))?;
    if null_buckets > 0 {
        bail!(
            "{file} has {null_buckets} row(s) with a null bucket — every row must be \
             'attributed' or carry a residual reason such as 'residual_no_provenance'"
        );
    }

    // These rows are already aggregated per (run_id, graph_id, node_index), so
    // ingesting the same run twice would double its GPU time.
    let already_ingested: Vec<String> = conn
        .prepare(&format!(
            "SELECT DISTINCT k.run_id FROM {prefix}sigiro_gpu_kernels k \
             WHERE k.run_id IN (SELECT DISTINCT run_id FROM {source}) ORDER BY k.run_id"
        ))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect()
        })
        .with_context(|| format!("failed to check {file} against already-ingested runs"))?;
    if !already_ingested.is_empty() {
        bail!(
            "sigiro_gpu_kernels already has rows for run(s) {} — ingesting {file} again would \
             double their GPU time; delete those rows first if this is a re-ingest",
            already_ingested.join(", ")
        );
    }

    conn.execute(
        &format!(
            "INSERT INTO {prefix}sigiro_gpu_kernels ({columns}) SELECT {columns} FROM {source}"
        ),
        [],
    )
    .with_context(|| {
        format!(
            "failed to insert {file} into sigiro_gpu_kernels — the Parquet file must have every \
             column of the table: {columns}"
        )
    })
}

/// Record one finished run in `sigiro_experiment_runs`.
pub fn record_run(conn: &Connection, record: &ExperimentRunRecord) -> Result<()> {
    for (name, timestamp) in [
        ("started_at", Some(record.started_at)),
        ("ended_at", record.ended_at),
    ] {
        if let Some(timestamp) = timestamp
            && !(MICROS_MIN..MICROS_MAX).contains(&timestamp)
        {
            bail!(
                "{} {name} ({timestamp}) is not a plausible 16-digit microsecond timestamp",
                record.run_id
            );
        }
    }
    if let Some(ended_at) = record.ended_at
        && ended_at < record.started_at
    {
        bail!("{} ended_at is before started_at", record.run_id);
    }

    let prefix = ProjectPool::prefix();
    let existing: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM {prefix}sigiro_experiment_runs \
                 WHERE experiment_id = ? AND run_id = ? AND arm = ?"
            ),
            params![&record.experiment_id, &record.run_id, &record.arm],
            |row| row.get(0),
        )
        .context("failed to check sigiro_experiment_runs for an existing run")?;
    if existing > 0 {
        bail!(
            "sigiro_experiment_runs already has run {} of experiment {} on arm {} — one row per \
             (experiment_id, run_id, arm); delete it first if this is a re-record",
            record.run_id,
            record.experiment_id,
            record.arm
        );
    }

    let config_json = record
        .config_json
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("failed to serialize config_json")?;

    conn.execute(
        &format!(
            "INSERT INTO {prefix}sigiro_experiment_runs (
                experiment_id, run_id, arm, parent_run_id, started_at, ended_at,
                config_sha, model_sha, hardware_sha, benchmark_sha,
                engine, engine_version, model_id, model_revision,
                gpu_model, gpu_count, tp_size, quantization, config_json,
                goodput, ttft_p95_ms, itl_ms, output_tokens_per_s,
                gpu_seconds, price_per_hour_usd, outcome, drift_detected
             ) VALUES (
                ?, ?, ?, ?, make_timestamp(CAST(? AS BIGINT)), make_timestamp(CAST(? AS BIGINT)),
                ?, ?, ?, ?,
                ?, ?, ?, ?,
                ?, ?, ?, ?, ?,
                ?, ?, ?, ?,
                ?, ?, ?, ?
             )"
        ),
        params![
            &record.experiment_id,
            &record.run_id,
            &record.arm,
            &record.parent_run_id,
            record.started_at,
            record.ended_at,
            &record.config_sha,
            &record.model_sha,
            &record.hardware_sha,
            &record.benchmark_sha,
            &record.engine,
            &record.engine_version,
            &record.model_id,
            &record.model_revision,
            &record.gpu_model,
            record.gpu_count,
            record.tp_size,
            &record.quantization,
            &config_json,
            record.goodput,
            record.ttft_p95_ms,
            record.itl_ms,
            record.output_tokens_per_s,
            record.gpu_seconds,
            record.price_per_hour_usd,
            &record.outcome,
            record.drift_detected,
        ],
    )
    .with_context(|| {
        format!(
            "failed to insert run {} into sigiro_experiment_runs",
            record.run_id
        )
    })?;
    Ok(())
}

/// Write one comparison's verdict onto every row of its experiment.
///
/// Returns the number of rows updated.
///
/// This exists because `record-run` writes a row the moment a period ends,
/// which is before anyone knows whether the change helped: the verdict comes
/// from `experiment::compare_experiment`, which reads all six periods at once
/// and runs only after the last one. Without this command `outcome` and
/// `drift_detected` are always NULL and the table cannot answer the one
/// question it exists for.
///
/// `outcome` and `drift_detected` are properties of the whole comparison, not
/// of one period — the A/B/B/A rule has no per-period verdict to give — so they
/// are written to EVERY row of the experiment. That denormalisation is
/// deliberate: "did this change help?" has to be answerable from
/// `sigiro_experiment_runs` alone, without a join to a second table, so the
/// verdict has to sit on the same row as the lineage edge and the goodput it
/// judges.
///
/// `drift_detected` is derived here rather than passed in, because
/// `ExperimentOutcome::DriftDetected` IS the drift verdict and a flag that can
/// disagree with the outcome next to it is a bug waiting to be queried.
///
/// Fails when the experiment has no rows — a verdict with nowhere to land is a
/// silent no-op — and when the outcome is not one of the values
/// `ExperimentOutcome` produces, which is checked by deserializing into that
/// enum so the accepted vocabulary has exactly one definition.
pub fn finalize_experiment(conn: &Connection, experiment_id: &str, outcome: &str) -> Result<usize> {
    let parsed: ExperimentOutcome = serde_json::from_value(serde_json::Value::String(
        outcome.to_string(),
    ))
    .with_context(|| format!("{outcome} is not a verdict /v1/experiments/compare can produce"))?;
    let drift_detected = matches!(parsed, ExperimentOutcome::DriftDetected);

    let prefix = ProjectPool::prefix();
    let rows: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {prefix}sigiro_experiment_runs WHERE experiment_id = ?"),
            params![experiment_id],
            |row| row.get(0),
        )
        .context("failed to check sigiro_experiment_runs for the experiment's rows")?;
    if rows == 0 {
        bail!(
            "sigiro_experiment_runs has no rows for experiment {experiment_id} — record its \
             periods with `sigiro record-run` first; finalizing an absent experiment would \
             report success while writing the verdict nowhere"
        );
    }

    conn.execute(
        &format!(
            "UPDATE {prefix}sigiro_experiment_runs \
             SET outcome = ?, drift_detected = ? WHERE experiment_id = ?"
        ),
        params![outcome, drift_detected, experiment_id],
    )
    .with_context(|| {
        format!("failed to write the {outcome} verdict onto experiment {experiment_id}")
    })
}
