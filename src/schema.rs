use anyhow::{Context, Result};
use duckdb::Connection;
use tracing::info;

/// Fleet observability extensions — loaded on each project connection.
/// Each extension provides SQL functions agents can query to debug system state.
pub const FLEET_EXTENSIONS: &[&str] = &["system_stats", "fsquery", "hostfs"];

/// Load a single extension: try LOAD from name first (uses extension_directory cache),
/// then try loading from bundled file path, then fall back to download.
fn load_one_extension(conn: &Connection, name: &str) -> Result<()> {
    // Path 1: LOAD by name — requires extension_directory set and extension pre-cached.
    if conn.execute_batch(&format!("LOAD {name};")).is_ok() {
        info!("loaded extension (cached): {name}");
        return Ok(());
    }

    // Path 2: LOAD from bundled file path — works for offline deployments where
    // extensions were shipped alongside the binary in {data_dir}/extensions/{name}.duckdb_extension
    let bundled = crate::config::DATA_DIR
        .value()
        .ok()
        .filter(|d| !d.is_empty())
        .map(|d| {
            std::path::Path::new(&d)
                .join("extensions")
                .join(format!("{name}.duckdb_extension"))
        });
    if let Some(ref path) = bundled {
        if path.exists() {
            if let Ok(()) = conn.execute_batch(&format!("LOAD '{}';", path.display())) {
                info!("loaded extension (bundled): {name}");
                return Ok(());
            }
        }
    }

    // Path 3: download from community repository, then LOAD — requires network
    conn.execute_batch(&format!("INSTALL {name} FROM community; LOAD {name};"))
        .with_context(|| format!(
            "failed to load extension '{name}' — run `sigiro fetch-extensions` to pre-download, or place {name}.duckdb_extension in $SIGIRO_DATA_DIR/extensions/"
        ))?;
    info!("loaded extension (downloaded): {name}");
    Ok(())
}

/// Load all fleet observability DuckDB community extensions.
/// Uses local cache when available, downloads only on first use.
/// Fails hard if ANY extension cannot be loaded — run `sigiro fetch-extensions` first.
pub fn load_fleet_extensions(conn: &Connection) -> Result<()> {
    for name in FLEET_EXTENSIONS {
        load_one_extension(conn, name)?;
    }
    Ok(())
}

/// Create all sigiro tables and views.
/// `prefix` is always "lake." (DuckLake catalog-qualified tables).
pub fn create_tables(conn: &Connection, prefix: &str) -> Result<()> {
    // Traces — 25 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_spans (
            timestamp             TIMESTAMP NOT NULL,
            end_timestamp         BIGINT NOT NULL,
            -- duration is stored in microseconds (not nanoseconds).
            -- The ingest path divides OTLP nanoseconds by 1_000_000 before writing.
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

    // Anomaly detection view — always in the default catalog (reads from prefixed tables)
    conn.execute_batch(&format!(
        "CREATE OR REPLACE VIEW sigiro_anomalies AS
        WITH error_rates AS (
            SELECT service_name,
                   count(CASE WHEN status_code = 2 THEN 1 END) * 100.0 / NULLIF(count(*), 0) AS error_pct
            FROM {prefix}sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'
            GROUP BY service_name
        ),
        latency AS (
            SELECT service_name,
                   approx_quantile(duration, 0.95) AS p95_ns
            FROM {prefix}sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'
            GROUP BY service_name
        ),
        silence AS (
            SELECT DISTINCT service_name FROM {prefix}sigiro_spans
            EXCEPT
            SELECT DISTINCT service_name FROM {prefix}sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'
        )
        SELECT 'error_rate' AS anomaly_type, service_name, error_pct AS value FROM error_rates WHERE error_pct > 5
        UNION ALL
        SELECT 'high_latency', service_name, CAST(p95_ns AS DOUBLE) FROM latency WHERE p95_ns > 1000000000
        UNION ALL
        SELECT 'silence', service_name, 0.0 FROM silence"
    ))
    .context("failed to create sigiro_anomalies view")?;
    // NOTE: DuckLake (Parquet-backed catalog) does not support CREATE INDEX.
    // Query performance relies on Parquet row group pruning and column statistics.
    // For heavy query workloads, consider partitioning by timestamp or using
    // DuckDB's native tables (non-DuckLake) for hot data.

    Ok(())
}
