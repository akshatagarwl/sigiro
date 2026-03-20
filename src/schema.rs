use anyhow::{Context, Result};
use duckdb::Connection;

pub fn create_tables(conn: &Connection) -> Result<()> {
    // Traces — 25 columns, matches otlp2records::traces_schema()
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_spans (
            timestamp             TIMESTAMP NOT NULL,
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
        )",
    )
    .context("failed to create sigiro_spans")?;

    // Logs — 15 columns, matches otlp2records::logs_schema()
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_logs (
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
            log_attributes        VARCHAR
        )",
    )
    .context("failed to create sigiro_logs")?;

    // Gauge — 16 columns, matches otlp2records::gauge_schema()
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_gauge (
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
        )",
    )
    .context("failed to create sigiro_metrics_gauge")?;

    // Sum — 18 columns, matches otlp2records::sum_schema()
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_sum (
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
        )",
    )
    .context("failed to create sigiro_metrics_sum")?;

    // Histogram — 22 columns, matches otlp2records::histogram_schema()
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_histogram (
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
        )",
    )
    .context("failed to create sigiro_metrics_histogram")?;

    // ExpHistogram — 27 columns, matches otlp2records::exp_histogram_schema()
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_metrics_exp_histogram (
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
        )",
    )
    .context("failed to create sigiro_metrics_exp_histogram")?;

    // Anomaly detection view — surfaces error rates, high latency, and silent services
    conn.execute_batch(
        "CREATE OR REPLACE VIEW sigiro_anomalies AS
        WITH error_rates AS (
            SELECT service_name,
                   count(*) FILTER (WHERE status_code = 2) * 100.0 / NULLIF(count(*), 0) AS error_pct
            FROM sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'
            GROUP BY service_name
        ),
        latency AS (
            SELECT service_name,
                   approx_quantile(duration, 0.95) AS p95_ns
            FROM sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'
            GROUP BY service_name
        ),
        silence AS (
            SELECT DISTINCT service_name FROM sigiro_spans
            EXCEPT
            SELECT DISTINCT service_name FROM sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'
        )
        SELECT 'error_rate' AS anomaly_type, service_name, error_pct AS value FROM error_rates WHERE error_pct > 5
        UNION ALL
        SELECT 'high_latency', service_name, CAST(p95_ns AS DOUBLE) FROM latency WHERE p95_ns > 1000000000
        UNION ALL
        SELECT 'silence', service_name, 0.0 FROM silence",
    )
    .context("failed to create sigiro_anomalies view")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_tables_on_fresh_db() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("create_tables should succeed");

        // Verify all tables exist
        let tables = [
            "sigiro_spans",
            "sigiro_logs",
            "sigiro_metrics_gauge",
            "sigiro_metrics_sum",
            "sigiro_metrics_histogram",
            "sigiro_metrics_exp_histogram",
        ];
        for table in tables {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap_or_else(|_| panic!("{table} should exist"));
            assert_eq!(count, 0, "{table} should be empty");
        }
    }

    #[test]
    fn create_tables_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("first call");
        create_tables(&conn).expect("second call should also succeed");
    }

    #[test]
    fn anomalies_view_exists() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_tables(&conn).expect("create_tables");
        // View should be queryable even with no data
        let count: i64 = conn
            .query_row("SELECT count(*) FROM sigiro_anomalies", [], |row| {
                row.get(0)
            })
            .expect("sigiro_anomalies view should be queryable");
        assert_eq!(count, 0);
    }
}
