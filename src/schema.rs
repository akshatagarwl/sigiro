use anyhow::{Context, Result};
use duckdb::{Connection, OptionalExt, params};

/// Create all sigiro tables and views.
/// `prefix` is always "lake." (DuckLake catalog-qualified tables).
pub fn create_tables(conn: &Connection, prefix: &str) -> Result<()> {
    validate_existing_timestamp_schema(conn, prefix)?;
    // Traces — 25 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_spans (
            timestamp             TIMESTAMPTZ NOT NULL,
            -- end_timestamp is an absolute UTC instant; duration is integer microseconds.
            -- The ingest path converts OTLP nanoseconds via `nanos_to_micros`
            -- (one checked named helper — never a bare divisor; see otlp.rs).
            end_timestamp         TIMESTAMPTZ NOT NULL,
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
            timestamp             TIMESTAMPTZ NOT NULL,
            observed_timestamp    TIMESTAMPTZ NOT NULL,
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
            template_values       VARCHAR,
            event_name            VARCHAR
        )"
    ))
    .context("failed to create sigiro_logs")?;

    // CREATE TABLE IF NOT EXISTS does not evolve existing tenant catalogs.
    conn.execute_batch(&format!(
        "ALTER TABLE {prefix}sigiro_logs ADD COLUMN IF NOT EXISTS event_name VARCHAR;
         UPDATE {prefix}sigiro_logs SET event_name = '' WHERE event_name IS NULL"
    ))
    .context("failed to add sigiro_logs.event_name")?;

    // Log template lookup table — maps template_id to template text.
    // Populated at insert time by the DRAIN algorithm.
    // No PRIMARY KEY — DuckLake does not support PK/UNIQUE constraints.
    // Duplicate template rows are harmless; dedup is done at insert time.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_log_templates (
            template_id           UBIGINT,
            template_text         VARCHAR NOT NULL,
            first_seen            TIMESTAMPTZ NOT NULL DEFAULT now()
        )"
    ))
    .context("failed to create sigiro_log_templates")?;

    // Gauge — 16 columns, direct Arrow builder (src/otlp.rs)
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {prefix}sigiro_metrics_gauge (
            timestamp             TIMESTAMPTZ NOT NULL,
            start_timestamp       TIMESTAMPTZ,
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
            timestamp                  TIMESTAMPTZ NOT NULL,
            start_timestamp            TIMESTAMPTZ,
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
            timestamp                  TIMESTAMPTZ NOT NULL,
            start_timestamp            TIMESTAMPTZ,
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
            timestamp                  TIMESTAMPTZ NOT NULL,
            start_timestamp            TIMESTAMPTZ,
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
            timestamp             TIMESTAMPTZ NOT NULL,
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
            detected_at           TIMESTAMPTZ NOT NULL,
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
            ts_shift              TIMESTAMPTZ NOT NULL,
            -- Series mean before/after the shift, in raw signal units
            -- (error %, p95 µs, logs per bucket, profile cost per bucket).
            \"before\"            DOUBLE NOT NULL,
            \"after\"             DOUBLE NOT NULL,
            -- Ready-to-run /v1/query SQL for the raw telemetry behind the
            -- shift (spans, logs, or profile samples, per the signal).
            drill_down_sql        VARCHAR NOT NULL
        )"
    ))
    .context("failed to create sigiro_anomalies")?;
    // NOTE: DuckLake (Parquet-backed catalog) does not support CREATE INDEX.
    // Query performance relies on Parquet row group pruning and column statistics.
    // For heavy query workloads, consider partitioning by timestamp or using
    // DuckDB's native tables (non-DuckLake) for hot data.

    Ok(())
}

/// Reject pre-TIMESTAMPTZ catalogs before any CREATE/ALTER statement runs.
///
/// DuckLake's `CREATE TABLE IF NOT EXISTS` does not evolve a tenant catalog,
/// and changing the physical type in place would make old Parquet files
/// ambiguous. The supported migration strategy is explicit version rejection:
/// operators must provision a new catalog (or restore the original catalog
/// from backup), rather than silently reading old timestamp semantics or
/// maintaining a dual read path.
fn validate_existing_timestamp_schema(conn: &Connection, prefix: &str) -> Result<()> {
    const REQUIRED: &[(&str, &[&str])] = &[
        ("sigiro_spans", &["timestamp", "end_timestamp"]),
        ("sigiro_logs", &["timestamp", "observed_timestamp"]),
        ("sigiro_log_templates", &["first_seen"]),
        ("sigiro_metrics_gauge", &["timestamp", "start_timestamp"]),
        ("sigiro_metrics_sum", &["timestamp", "start_timestamp"]),
        (
            "sigiro_metrics_histogram",
            &["timestamp", "start_timestamp"],
        ),
        (
            "sigiro_metrics_exp_histogram",
            &["timestamp", "start_timestamp"],
        ),
        ("sigiro_profiles", &["timestamp"]),
        ("sigiro_anomalies", &["detected_at", "ts_shift"]),
    ];

    let (catalog, schema) = if prefix.is_empty() {
        conn.query_row("SELECT current_catalog(), current_schema()", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to identify current DuckDB catalog and schema")?
    } else {
        let catalog = prefix.strip_suffix('.').with_context(|| {
            format!("invalid schema prefix {prefix:?}; expected catalog-qualified prefix")
        })?;
        (catalog.to_string(), "main".to_string())
    };

    let exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM information_schema.tables
             WHERE table_catalog = ? AND table_schema = ? AND table_name LIKE 'sigiro_%'",
            params![catalog, schema],
            |row| row.get(0),
        )
        .context("failed to inspect existing DuckLake schema")?;
    if exists == 0 {
        return Ok(());
    }

    if exists != REQUIRED.len() as i64 {
        anyhow::bail!(
            "unsupported partial DuckLake catalog schema: found {exists} sigiro tables, expected {}; provision a new catalog",
            REQUIRED.len()
        );
    }

    for (table, columns) in REQUIRED {
        let table_name = format!("{prefix}{table}");
        for column in *columns {
            let actual: Option<String> = conn
                .query_row(
                    "SELECT data_type FROM information_schema.columns
                     WHERE table_catalog = ? AND table_schema = ?
                       AND table_name = ? AND column_name = ?",
                    params![catalog, schema, table, column],
                    |row| row.get(0),
                )
                .optional()
                .with_context(|| format!("failed to inspect {table_name}.{column}"))?;
            let actual = actual.with_context(|| format!(
                "DuckLake catalog is missing required absolute timestamp column {table_name}.{column}; catalog version is unsupported"
            ))?;
            if actual != "TIMESTAMP WITH TIME ZONE" {
                anyhow::bail!(
                    "unsupported DuckLake catalog schema: {table_name}.{column} is {actual}, expected TIMESTAMPTZ; migrate by provisioning a new catalog"
                );
            }
        }
    }
    Ok(())
}
