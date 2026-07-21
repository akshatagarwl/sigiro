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
    // driven from the per-project inserter timer), read by /v1/investigate
    // and /v1/anomalies. Detection is BOCPD changepoints (augurs) over
    // continuous per-operation series — NO arbitrary thresholds anywhere
    // (docs/plans/01-continuous-anomaly-subsystem.md). Lives in the DuckLake
    // catalog alongside the telemetry it derives from, so it gets the same
    // per-tenant isolation for free.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_anomalies (
            detected_at           TIMESTAMP NOT NULL,
            service_name          VARCHAR NOT NULL,
            operation             VARCHAR NOT NULL,
            -- Which health series shifted; the spans slice emits 'error_rate'.
            signal                VARCHAR NOT NULL,
            -- Detection kind; BOCPD emits 'changepoint'.
            kind                  VARCHAR NOT NULL,
            -- Only 'worsening' shifts are persisted (mean after > mean
            -- before — a direction comparison, not a threshold).
            direction             VARCHAR NOT NULL,
            -- First bucket of the NEW regime.
            ts_shift              TIMESTAMP NOT NULL,
            -- Series mean before/after the shift, in raw units (error %).
            \"before\"            DOUBLE NOT NULL,
            \"after\"             DOUBLE NOT NULL,
            -- Ready-to-run /v1/query SQL for the spans behind the shift.
            drill_down_sql        VARCHAR
        )"
    ))
    .context("failed to create sigiro_anomalies")?;
    // NOTE: DuckLake (Parquet-backed catalog) does not support CREATE INDEX.
    // Query performance relies on Parquet row group pruning and column statistics.
    // For heavy query workloads, consider partitioning by timestamp or using
    // DuckDB's native tables (non-DuckLake) for hot data.

    Ok(())
}
