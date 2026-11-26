use anyhow::{Context, Result};
use duckdb::Connection;

/// Create all sigiro tables. The statements below are the whole schema: there
/// are no migrations, and a data directory written by an older schema is
/// rejected when opened rather than upgraded.
/// `prefix` is always "lake." (DuckLake catalog-qualified tables).
pub fn create_tables(conn: &Connection, prefix: &str) -> Result<()> {
    declare_tables(conn, prefix)?;
    let catalog = prefix.trim_end_matches('.');
    if catalog.is_empty() {
        return Ok(());
    }
    let declared = crate::pool::open_duckdb()?;
    declare_tables(&declared, "")?;
    let expected = columns(&declared, "memory")?;
    let actual = columns(conn, catalog)?;
    anyhow::ensure!(
        actual == expected,
        "the data directory was written by a different sigiro schema; \
         delete it to start over (sigiro does not migrate data)"
    );
    Ok(())
}

/// (table, column, type) for every sigiro table in `catalog`, in column order.
fn columns(conn: &Connection, catalog: &str) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT table_name, column_name, data_type FROM information_schema.columns
         WHERE table_catalog = ? AND table_name LIKE 'sigiro\\_%' ESCAPE '\\'
         ORDER BY table_name, ordinal_position",
    )?;
    let rows = stmt.query_map([catalog], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect::<Result<_, _>>()
        .context("failed to read the stored schema")
}

fn declare_tables(conn: &Connection, prefix: &str) -> Result<()> {
    // Traces: 25 columns matching sigiro-segments.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_spans (
            timestamp             TIMESTAMP NOT NULL,
            -- Every point in time is a TIMESTAMP (microseconds, UTC); every
            -- length of time, like duration, is a BIGINT count of microseconds.
            -- Ingest truncates OTLP nanoseconds to microseconds.
            end_timestamp         TIMESTAMP NOT NULL,
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
            flags                 INTEGER,
            span_attribute_refs   VARCHAR
        )"
    ))
    .context("failed to create sigiro_spans")?;

    // Span attribute values of 1 KiB or more, once per value per day. The
    // timestamp is the last microsecond of the day of the spans that refer to
    // the value, so retention keeps it as long as any of them.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_span_values (
            timestamp             TIMESTAMP NOT NULL,
            hash                  VARCHAR NOT NULL,
            value                 VARCHAR NOT NULL
        )"
    ))
    .context("failed to create sigiro_span_values")?;

    // Logs — OTLP fields plus DRAIN classification stored on the same row.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_logs (
            timestamp             TIMESTAMP NOT NULL,
            observed_timestamp    TIMESTAMP NOT NULL,
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
            start_timestamp       TIMESTAMP,
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
            start_timestamp            TIMESTAMP,
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
            start_timestamp            TIMESTAMP,
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
            start_timestamp            TIMESTAMP,
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
        assert!(tables.iter().any(|name| name == "sigiro_span_values"));
        assert!(tables.iter().any(|name| name == "sigiro_logs"));
        assert!(tables.iter().any(|name| name == "sigiro_profiles"));
        assert!(!tables.iter().any(|name| name == "sigiro_anomalies"));
    }

    /// One time type everywhere (#236): a point in time is a `TIMESTAMP` and
    /// is named `*timestamp`, so `/v1/query` compares and subtracts any two of
    /// them without a cast.
    #[test]
    fn every_point_in_time_is_a_timestamp() {
        let conn = crate::pool::open_duckdb().unwrap();
        create_tables(&conn, "").unwrap();
        let columns = columns(&conn, "memory").unwrap();
        let mismatched: Vec<_> = columns
            .iter()
            .filter(|(_, column, data_type)| {
                column.ends_with("timestamp") != (data_type == "TIMESTAMP")
            })
            .collect();
        assert!(mismatched.is_empty(), "{mismatched:?}");
        assert!(
            columns
                .iter()
                .any(|(table, column, _)| { table == "sigiro_spans" && column == "end_timestamp" })
        );
    }

    /// The queries from #236: a µs window over `timestamp`, and span length
    /// as `end_timestamp - timestamp`, pass the `/v1/query` firewall and run.
    #[test]
    fn a_span_window_and_length_need_no_cast() {
        let conn = crate::pool::open_duckdb().unwrap();
        create_tables(&conn, "").unwrap();
        conn.execute_batch(
            "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, span_name, service_name, span_kind, status_code) \
             VALUES (make_timestamp(1795604408000000), make_timestamp(1795604408250000), 250000, 'GET /', 'web', 2, 0)",
        )
        .unwrap();
        let sql = "SELECT end_timestamp - timestamp AS length, end_timestamp > timestamp AS ordered \
                   FROM sigiro_spans \
                   WHERE timestamp >= make_timestamp(1795604408000000) \
                   AND timestamp < make_timestamp(1795608008000000)";
        sigiro_segments::validate_read_only(sql).unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let stmt = row.as_ref();
        let values: Vec<_> = (0..2)
            .map(|i| {
                crate::duckdb_json::duckdb_row_value_to_json(row, i, &stmt.column_type(i)).unwrap()
            })
            .collect();
        assert_eq!(
            values,
            [serde_json::json!(250_000), serde_json::json!(true)]
        );
    }

    #[test]
    fn a_data_directory_from_another_schema_is_rejected() {
        let conn = crate::pool::open_duckdb().unwrap();
        conn.execute_batch("ATTACH ':memory:' AS lake").unwrap();
        create_tables(&conn, "lake.").unwrap();
        create_tables(&conn, "lake.").unwrap();

        conn.execute_batch("ALTER TABLE lake.sigiro_spans DROP COLUMN span_attribute_refs")
            .unwrap();
        let error = create_tables(&conn, "lake.").unwrap_err();
        assert!(error.to_string().contains("different sigiro schema"));
    }
}
