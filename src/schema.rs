use anyhow::{Context, Result};
use duckdb::Connection;

pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_spans (
            start_time_unix_nano  BIGINT NOT NULL,
            end_time_unix_nano    BIGINT NOT NULL,
            trace_id              VARCHAR NOT NULL,
            span_id               VARCHAR NOT NULL,
            parent_span_id        VARCHAR,
            span_name             VARCHAR NOT NULL,
            span_kind             INTEGER NOT NULL,
            status_code           INTEGER NOT NULL,
            status_message        VARCHAR,
            service_name          VARCHAR NOT NULL,
            duration_nano         BIGINT NOT NULL,
            resource_attributes   VARCHAR,
            span_attributes       VARCHAR,
            events_json           VARCHAR,
            links_json            VARCHAR
        )",
    )
    .context("failed to create sigiro_spans")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_logs (
            timestamp_unix_nano    BIGINT NOT NULL,
            observed_timestamp     BIGINT,
            trace_id               VARCHAR,
            span_id                VARCHAR,
            severity_number        INTEGER,
            severity_text          VARCHAR,
            body                   VARCHAR,
            service_name           VARCHAR NOT NULL,
            resource_attributes    VARCHAR,
            log_attributes         VARCHAR
        )",
    )
    .context("failed to create sigiro_logs")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_gauge (
            metric_name            VARCHAR NOT NULL,
            start_time_unix_nano   BIGINT,
            time_unix_nano         BIGINT NOT NULL,
            gauge_value            DOUBLE,
            exemplar_json          VARCHAR,
            flags                  INTEGER,
            service_name           VARCHAR NOT NULL,
            resource_attributes    VARCHAR,
            metric_attributes      VARCHAR
        )",
    )
    .context("failed to create sigiro_metrics_gauge")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_sum (
            metric_name                VARCHAR NOT NULL,
            start_time_unix_nano       BIGINT,
            time_unix_nano             BIGINT NOT NULL,
            sum_value                  DOUBLE,
            exemplar_json              VARCHAR,
            flags                      INTEGER,
            is_monotonic               BOOLEAN,
            aggregation_temporality    INTEGER,
            service_name               VARCHAR NOT NULL,
            resource_attributes        VARCHAR,
            metric_attributes          VARCHAR
        )",
    )
    .context("failed to create sigiro_metrics_sum")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_histogram (
            metric_name                VARCHAR NOT NULL,
            start_time_unix_nano       BIGINT,
            time_unix_nano             BIGINT NOT NULL,
            histogram_count            BIGINT,
            histogram_sum              DOUBLE,
            histogram_min              DOUBLE,
            histogram_max              DOUBLE,
            bucket_counts              VARCHAR,
            explicit_bounds            VARCHAR,
            exemplar_json              VARCHAR,
            flags                      INTEGER,
            aggregation_temporality    INTEGER,
            service_name               VARCHAR NOT NULL,
            resource_attributes        VARCHAR,
            metric_attributes          VARCHAR
        )",
    )
    .context("failed to create sigiro_metrics_histogram")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_exp_histogram (
            metric_name                VARCHAR NOT NULL,
            start_time_unix_nano       BIGINT,
            time_unix_nano             BIGINT NOT NULL,
            exp_histogram_count        BIGINT,
            exp_histogram_sum          DOUBLE,
            exp_histogram_min          DOUBLE,
            exp_histogram_max          DOUBLE,
            exp_histogram_scale        INTEGER,
            exp_histogram_zero_count   BIGINT,
            positive_offset            INTEGER,
            positive_bucket_counts     VARCHAR,
            negative_offset            INTEGER,
            negative_bucket_counts     VARCHAR,
            exemplar_json              VARCHAR,
            flags                      INTEGER,
            aggregation_temporality    INTEGER,
            service_name               VARCHAR NOT NULL,
            resource_attributes        VARCHAR,
            metric_attributes          VARCHAR
        )",
    )
    .context("failed to create sigiro_metrics_exp_histogram")?;

    Ok(())
}
