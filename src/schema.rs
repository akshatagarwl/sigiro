use anyhow::{Context, Result};
use duckdb::Connection;

/// Create all sigiro tables and views.
/// `prefix` is always "lake." (DuckLake catalog-qualified tables).
pub fn create_tables(conn: &Connection, prefix: &str) -> Result<()> {
    // Traces: 25 columns matching sigiro-segments.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_spans (
            timestamp             TIMESTAMP NOT NULL,
            -- end_timestamp and duration are microseconds, matching `timestamp`.
            -- Ingest stores OTLP nanoseconds as microseconds.
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

    // Logs — OTLP fields plus DRAIN classification stored on the same row.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_logs (
            timestamp             TIMESTAMP NOT NULL,
            observed_timestamp    BIGINT NOT NULL,
            trace_id              VARCHAR,
            span_id               VARCHAR,
            event_name            VARCHAR,
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
            template_values       VARCHAR,
            template_text         VARCHAR
        )"
    ))
    .context("failed to create sigiro_logs")?;

    // Gauge: 16 columns matching sigiro-segments.
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

    // Sum: 18 columns matching sigiro-segments.
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

    // Histogram: 22 columns matching sigiro-segments.
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

    // ExpHistogram: 27 columns matching sigiro-segments.
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
            resource_attributes   VARCHAR,
            labels                VARCHAR
        )"
    ))
    .context("failed to create sigiro_profiles")?;
    // Tables created before per-sample labels existed gain the column at the
    // end, where the positional appender writes it.
    conn.execute_batch(&format!(
        "ALTER TABLE {prefix}sigiro_profiles ADD COLUMN IF NOT EXISTS labels VARCHAR"
    ))
    .context("failed to add sigiro_profiles.labels")?;

    // NOTE: DuckLake (Parquet-backed catalog) does not support CREATE INDEX.
    // Query performance relies on Parquet row group pruning and column statistics.
    // For heavy query workloads, consider partitioning by timestamp or using
    // DuckDB's native tables (non-DuckLake) for hot data.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_schema_has_raw_telemetry_but_no_saved_anomalies() {
        let conn = crate::pool::open_duckdb().unwrap();
        create_tables(&conn, "").unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT table_name FROM information_schema.tables WHERE table_schema = 'main'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert!(tables.iter().any(|name| name == "sigiro_spans"));
        assert!(tables.iter().any(|name| name == "sigiro_logs"));
        assert!(tables.iter().any(|name| name == "sigiro_profiles"));
        assert!(!tables.iter().any(|name| name == "sigiro_anomalies"));
    }
}
