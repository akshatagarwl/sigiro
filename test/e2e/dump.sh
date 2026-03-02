#!/usr/bin/env bash
# Formatted data dump for manual inspection after E2E run.
# Prereq: MinIO must be running with data at s3://observe/**/*.parquet
# Usage: ./dump.sh

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# DuckDB: prefer local CLI, fall back to Docker
if command -v duckdb &>/dev/null; then
  DUCKDB="duckdb"
else
  echo "duckdb CLI not found, using Docker image"
  DUCKDB="docker run --rm --network host datacoves/duckdb duckdb"
fi

S3_SETUP="
INSTALL httpfs; LOAD httpfs;
SET s3_endpoint='localhost:9000';
SET s3_access_key_id='minioadmin';
SET s3_secret_access_key='minioadmin';
SET s3_use_ssl=false;
SET s3_url_style='path';
"
PARQUET="read_parquet('s3://observe/**/*.parquet')"

run_query() {
  local title="$1"
  local query="$2"
  echo "=== $title ==="
  $DUCKDB -markdown -c "${S3_SETUP} ${query}" 2>&1 || true
  echo ""
}

run_query "File Inventory" "
SELECT file_name, count(*) AS row_groups
FROM parquet_metadata('s3://observe/**/*.parquet')
GROUP BY file_name
ORDER BY file_name;
"

run_query "Overview" "
SELECT signal, service, count(*) AS rows,
       min(timestamp) AS min_ts, max(timestamp) AS max_ts
FROM $PARQUET
GROUP BY signal, service
ORDER BY service, signal;
"

run_query "Metrics — Names and Value Ranges" "
SELECT name, count(*) AS n, round(min(value),2) AS min_val,
       round(avg(value),2) AS avg_val, round(max(value),2) AS max_val
FROM $PARQUET WHERE signal='metric'
GROUP BY name ORDER BY name;
"

run_query "Metrics — Sample Rows (5)" "
SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M:%S') AS t,
       service, name, round(value,2) AS value
FROM $PARQUET WHERE signal='metric'
ORDER BY timestamp LIMIT 5;
"

run_query "Spans — Status and Duration Distribution" "
SELECT service, status, count(*) AS n,
       round(min(value),1) AS min_ms, round(avg(value),1) AS avg_ms,
       round(max(value),1) AS max_ms
FROM $PARQUET WHERE signal='span'
GROUP BY service, status ORDER BY service, status;
"

run_query "Spans — Sample Rows (5, slowest first)" "
SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M:%S') AS t,
       service, name, round(value,1) AS duration_ms,
       status, trace_id[:16] AS trace_id_prefix
FROM $PARQUET WHERE signal='span'
ORDER BY value DESC LIMIT 5;
"

run_query "Logs — Severity Distribution" "
SELECT service, status, count(*) AS n
FROM $PARQUET WHERE signal='log'
GROUP BY service, status ORDER BY service, status;
"

run_query "Logs — Sample Rows (5, most recent)" "
SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M:%S') AS t,
       service, status, message[:80] AS message
FROM $PARQUET WHERE signal='log'
ORDER BY timestamp DESC LIMIT 5;
"

run_query "Profiles — Top Functions" "
SELECT service, stacktrace[len(stacktrace)] AS leaf_function,
       round(sum(value),0) AS total_samples
FROM $PARQUET WHERE signal='profile' AND len(stacktrace) > 0
GROUP BY service, leaf_function
ORDER BY total_samples DESC LIMIT 10;
"

run_query "Profiles — Sample Stacktraces (3)" "
SELECT service, stacktrace, round(value,0) AS samples
FROM $PARQUET WHERE signal='profile' AND len(stacktrace) > 0
LIMIT 3;
"

run_query "Cross-Service Traces" "
SELECT trace_id[:16] AS trace_prefix, count(DISTINCT service) AS services,
       list(DISTINCT service) AS service_list, count(*) AS spans
FROM $PARQUET WHERE signal='span' AND trace_id IS NOT NULL
GROUP BY trace_id HAVING count(DISTINCT service) > 1
LIMIT 5;
"

run_query "Sort Order Check (first 20 rows)" "
SELECT service, signal, strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M:%S') AS t
FROM (
  SELECT *, row_number() OVER () AS rn
  FROM $PARQUET
) ORDER BY rn LIMIT 20;
"

exit 0
