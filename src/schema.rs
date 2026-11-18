use anyhow::{Context, Result};
use duckdb::Connection;

/// Create all sigiro tables and views.
/// `prefix` is always "lake." (DuckLake catalog-qualified tables).
pub fn create_tables(conn: &Connection, prefix: &str) -> Result<()> {
    // Traces — 25 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_spans (
            timestamp             TIMESTAMP NOT NULL,
            -- end_timestamp and duration are microseconds, matching `timestamp`.
            -- The ingest path converts OTLP nanoseconds via `nanos_to_micros`
            -- (one named helper — never a bare divisor; see otlp.rs).
            end_timestamp         BIGINT NOT NULL,
            duration              BIGINT NOT NULL,
            trace_id              VARCHAR,
            span_id               VARCHAR,
            parent_span_id        VARCHAR,
            trace_state           VARCHAR,
            service_name          VARCHAR NOT NULL,
            service_namespace     VARCHAR,
            service_instance_id   VARCHAR,
            span_name             VARCHAR NOT NULL,
            span_kind             INTEGER NOT NULL,
            status_code           INTEGER NOT NULL,
            status_message        VARCHAR,
            resource_attributes   VARCHAR,
            scope_name            VARCHAR,
            scope_version         VARCHAR,
            scope_attributes      VARCHAR,
            span_attributes       VARCHAR,
            events_json           VARCHAR,
            links_json            VARCHAR,
            dropped_attributes_count INTEGER,
            dropped_events_count  INTEGER,
            dropped_links_count   INTEGER,
            flags                 INTEGER
        )"
    ))
    .context("failed to create sigiro_spans")?;

    // Logs — 15 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_logs (
            timestamp             TIMESTAMP NOT NULL,
            observed_timestamp    BIGINT NOT NULL,
            trace_id              VARCHAR,
            span_id               VARCHAR,
            service_name          VARCHAR NOT NULL,
            service_namespace     VARCHAR,
            service_instance_id   VARCHAR,
            severity_number       INTEGER NOT NULL,
            severity_text         VARCHAR NOT NULL,
            body                  VARCHAR,
            resource_attributes   VARCHAR,
            scope_name            VARCHAR,
            scope_version         VARCHAR,
            scope_attributes      VARCHAR,
            log_attributes        VARCHAR,
            template_id           UBIGINT,
            template_values       VARCHAR
        )"
    ))
    .context("failed to create sigiro_logs")?;

    // Log template lookup table — maps template_id to template text.
    // Populated at insert time by the DRAIN algorithm.
    // No PRIMARY KEY — DuckLake does not support PK/UNIQUE constraints.
    // Duplicate template rows are harmless; dedup is done at insert time.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_log_templates (
            template_id           UBIGINT,
            template_text         VARCHAR NOT NULL,
            first_seen            TIMESTAMP NOT NULL DEFAULT now()
        )"
    ))
    .context("failed to create sigiro_log_templates")?;

    // Gauge — 16 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_metrics_gauge (
            timestamp             TIMESTAMP NOT NULL,
            start_timestamp       BIGINT,
            metric_name           VARCHAR NOT NULL,
            metric_description    VARCHAR,
            metric_unit           VARCHAR,
            value                 DOUBLE NOT NULL,
            service_name          VARCHAR NOT NULL,
            service_namespace     VARCHAR,
            service_instance_id   VARCHAR,
            resource_attributes   VARCHAR,
            scope_name            VARCHAR,
            scope_version         VARCHAR,
            scope_attributes      VARCHAR,
            metric_attributes     VARCHAR,
            flags                 INTEGER,
            exemplars_json        VARCHAR
        )"
    ))
    .context("failed to create sigiro_metrics_gauge")?;

    // Sum — 18 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_metrics_sum (
            timestamp                  TIMESTAMP NOT NULL,
            start_timestamp            BIGINT,
            metric_name                VARCHAR NOT NULL,
            metric_description         VARCHAR,
            metric_unit                VARCHAR,
            value                      DOUBLE NOT NULL,
            service_name               VARCHAR NOT NULL,
            service_namespace          VARCHAR,
            service_instance_id        VARCHAR,
            resource_attributes        VARCHAR,
            scope_name                 VARCHAR,
            scope_version              VARCHAR,
            scope_attributes           VARCHAR,
            metric_attributes          VARCHAR,
            flags                      INTEGER,
            exemplars_json             VARCHAR,
            aggregation_temporality    INTEGER NOT NULL,
            is_monotonic               BOOLEAN NOT NULL
        )"
    ))
    .context("failed to create sigiro_metrics_sum")?;

    // Histogram — 22 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_metrics_histogram (
            timestamp                  TIMESTAMP NOT NULL,
            start_timestamp            BIGINT,
            metric_name                VARCHAR NOT NULL,
            metric_description         VARCHAR,
            metric_unit                VARCHAR,
            count                      BIGINT NOT NULL,
            sum                        DOUBLE,
            min                        DOUBLE,
            max                        DOUBLE,
            bucket_counts              VARCHAR NOT NULL,
            explicit_bounds            VARCHAR NOT NULL,
            service_name               VARCHAR NOT NULL,
            service_namespace          VARCHAR,
            service_instance_id        VARCHAR,
            resource_attributes        VARCHAR,
            scope_name                 VARCHAR,
            scope_version              VARCHAR,
            scope_attributes           VARCHAR,
            metric_attributes          VARCHAR,
            flags                      INTEGER,
            exemplars_json             VARCHAR,
            aggregation_temporality    INTEGER NOT NULL
        )"
    ))
    .context("failed to create sigiro_metrics_histogram")?;

    // ExpHistogram — 27 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_metrics_exp_histogram (
            timestamp                  TIMESTAMP NOT NULL,
            start_timestamp            BIGINT,
            metric_name                VARCHAR NOT NULL,
            metric_description         VARCHAR,
            metric_unit                VARCHAR,
            count                      BIGINT NOT NULL,
            sum                        DOUBLE,
            min                        DOUBLE,
            max                        DOUBLE,
            scale                      INTEGER NOT NULL,
            zero_count                 BIGINT NOT NULL,
            zero_threshold             DOUBLE,
            positive_offset            INTEGER,
            positive_bucket_counts     VARCHAR,
            negative_offset            INTEGER,
            negative_bucket_counts     VARCHAR,
            service_name               VARCHAR NOT NULL,
            service_namespace          VARCHAR,
            service_instance_id        VARCHAR,
            resource_attributes        VARCHAR,
            scope_name                 VARCHAR,
            scope_version              VARCHAR,
            scope_attributes           VARCHAR,
            metric_attributes          VARCHAR,
            flags                      INTEGER,
            exemplars_json             VARCHAR,
            aggregation_temporality    INTEGER NOT NULL
        )"
    ))
    .context("failed to create sigiro_metrics_exp_histogram")?;

    // Profiles — one row per sample per value type.
    // Stack traces resolved from OTLP pprofile dictionary encoding at ingest time.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_profiles (
            profile_id            VARCHAR,
            trace_id              VARCHAR,
            span_id               VARCHAR,
            service_name          VARCHAR NOT NULL,
            timestamp             TIMESTAMP NOT NULL,
            profile_type          VARCHAR,
            stack_trace           VARCHAR[],
            value                 BIGINT,
            unit                  VARCHAR,
            resource_attributes   VARCHAR
        )"
    ))
    .context("failed to create sigiro_profiles")?;

    // Anomalies — written by the scheduled anomaly pass (src/anomaly.rs,
    // driven from the per-project inserter timer), read by /v1/diagnose
    // and /v1/anomalies. Detection is BOCPD changepoints (augurs) over
    // continuous 5-minute series of every signal — NO arbitrary thresholds
    // anywhere (docs/plans/01-continuous-anomaly-subsystem.md). Lives in the
    // DuckLake catalog alongside the telemetry it derives from, so it gets
    // the same per-tenant isolation for free.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_anomalies (
            detected_at           TIMESTAMP NOT NULL,
            service_name          VARCHAR NOT NULL,
            -- The entity the shifted series is keyed by: the derived
            -- operation identity (error_rate / latency_p95) or the service
            -- name (log_volume / error_log_rate / profile_cost).
            operation             VARCHAR NOT NULL,
            -- Which health series shifted: 'error_rate', 'latency_p95',
            -- 'log_volume', 'error_log_rate', or 'profile_cost'.
            signal                VARCHAR NOT NULL,
            -- Detection kind; BOCPD emits 'changepoint'.
            kind                  VARCHAR NOT NULL,
            -- Only 'worsening' shifts are persisted (mean after > mean
            -- before — a direction comparison, not a threshold).
            direction             VARCHAR NOT NULL,
            -- First bucket of the NEW regime.
            ts_shift              TIMESTAMP NOT NULL,
            -- Series mean before/after the shift, in raw signal units
            -- (error %, p95 µs, logs per bucket, profile cost per bucket).
            \"before\"            DOUBLE NOT NULL,
            \"after\"             DOUBLE NOT NULL,
            -- Ready-to-run /v1/query SQL for the raw telemetry behind the
            -- shift (spans, logs, or profile samples, per the signal).
            drill_down_sql        VARCHAR
        )"
    ))
    .context("failed to create sigiro_anomalies")?;

    // GPU kernels — the per-run kernel ledger. Written by the batch
    // `sigiro ingest-kernels` CLI (src/ledger.rs) from a profiler's Parquet
    // export, read by whoever asks "where did this run's GPU time go".
    //
    // One row per (run_id, graph_id, node_index): already aggregated by the
    // producer, because a per-launch table for a 10-minute benchmark is
    // hundreds of millions of rows nobody queries twice. There is no OTLP
    // path here on purpose — kernel timing arrives once, after a run ends.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_gpu_kernels (
            run_id                VARCHAR NOT NULL,
            experiment_id         VARCHAR,
            arm                   VARCHAR,
            timestamp             TIMESTAMP NOT NULL,
            service_name          VARCHAR NOT NULL,
            device                VARCHAR,
            rank                  INTEGER,
            -- Producer-side identity of the compiled graph and the node
            -- inside it. Null when the engine runs eager, not compiled.
            graph_id              BIGINT,
            node_index            BIGINT,
            kernel                VARCHAR NOT NULL,
            -- Model-level provenance, when the producer could resolve it.
            layer                 VARCHAR,
            op                    VARCHAR,
            attribution_method    VARCHAR,
            -- 'attributed', or a residual reason such as
            -- 'residual_no_provenance'. NEVER null: GPU time that could not
            -- be attributed has to be a row rather than an absence, or the
            -- ledger silently under-reports and every percentage computed
            -- from it is wrong.
            bucket                VARCHAR NOT NULL,
            executions            BIGINT NOT NULL,
            total_ns              BIGINT NOT NULL,
            mean_ns               BIGINT,
            max_ns                BIGINT,
            -- Occupancy inputs, when the profiler collected them.
            occupancy_pct         DOUBLE,
            registers_per_thread  INTEGER,
            shared_mem_bytes      BIGINT,
            limiting_factor       VARCHAR,
            -- Which producer nodes folded into this row.
            source_nodes          VARCHAR
        )"
    ))
    .context("failed to create sigiro_gpu_kernels")?;

    // Experiment runs — the run ledger. Written by the batch
    // `sigiro record-run` CLI (src/ledger.rs) once a run finishes, read by
    // the experiment comparison surface (src/experiment.rs) and by anyone
    // joining kernel rows back to the configuration that produced them.
    //
    // One row per (experiment_id, run_id, arm). The fingerprint columns are
    // what make two runs comparable at all; the outcome columns are the
    // verdict AIPerf and the comparison rule already produced, recorded here
    // so the verdict survives the process that computed it.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_experiment_runs (
            experiment_id         VARCHAR NOT NULL,
            run_id                VARCHAR NOT NULL,
            arm                   VARCHAR NOT NULL,
            -- Lineage edge: the run this one was derived from. This is what
            -- makes \"what changed over time\" a recursive query instead of a
            -- reconstruction from timestamps.
            parent_run_id         VARCHAR,
            started_at            TIMESTAMP NOT NULL,
            ended_at              TIMESTAMP,
            -- Identity of everything that must hold constant for a
            -- comparison to mean anything.
            config_sha            VARCHAR,
            model_sha             VARCHAR,
            hardware_sha          VARCHAR,
            benchmark_sha         VARCHAR,
            engine                VARCHAR,
            engine_version        VARCHAR,
            model_id              VARCHAR,
            model_revision        VARCHAR,
            gpu_model             VARCHAR,
            gpu_count             INTEGER,
            tp_size               INTEGER,
            quantization          VARCHAR,
            -- The full runtime configuration as JSON text, so a run stays
            -- readable after the knobs above stop being the right columns.
            config_json           VARCHAR,
            -- Results, in the units the benchmark reports them.
            goodput               DOUBLE,
            ttft_p95_ms           DOUBLE,
            itl_ms                DOUBLE,
            output_tokens_per_s   DOUBLE,
            gpu_seconds           DOUBLE,
            price_per_hour_usd    DOUBLE,
            -- Verdict for this run: 'confirmed', 'rejected', 'inconclusive',
            -- 'drift_detected' — the same vocabulary /v1/experiments/compare
            -- returns (src/experiment.rs).
            outcome               VARCHAR,
            drift_detected        BOOLEAN
        )"
    ))
    .context("failed to create sigiro_experiment_runs")?;
    // NOTE: DuckLake (Parquet-backed catalog) does not support CREATE INDEX.
    // Query performance relies on Parquet row group pruning and column statistics.
    // For heavy query workloads, consider partitioning by timestamp or using
    // DuckDB's native tables (non-DuckLake) for hot data.

    Ok(())
}
