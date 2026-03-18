#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

export DOCKER_BUILDKIT=1

# Cleanup on exit
cleanup() {
  echo "=== Cleaning up ==="
  docker compose down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Building images ==="
docker compose build

echo "=== Starting MinIO + Collector ==="
docker compose up -d minio minio-setup collector
echo "Waiting for collector to be ready..."
sleep 5

echo "=== Starting telemetry generators ==="
docker compose up -d \
  telemetrygen-checkout-traces \
  telemetrygen-checkout-metrics \
  telemetrygen-checkout-logs \
  telemetrygen-payments-traces \
  telemetrygen-payments-metrics \
  telemetrygen-payments-logs \
  profilegen

echo "=== Waiting for all signals to flush ==="
# Profiles bypass the batch processor and flush immediately.
# Traces/metrics/logs go through the 60s batch processor, so we must
# wait for at least one non-profile flush before running checks.
TIMEOUT=120
ELAPSED=0
while [ $ELAPSED -lt $TIMEOUT ]; do
  # Look for flushes of any non-profile signal
  NON_PROFILE=$(docker compose logs collector 2>&1 | grep "flushed parquet" | grep -c -v "signal=profile" || true)
  if [ "$NON_PROFILE" -gt 0 ]; then
    echo "Found $NON_PROFILE non-profile flush(es) after ${ELAPSED}s"
    break
  fi
  sleep 5
  ELAPSED=$((ELAPSED + 5))
done

if [ $ELAPSED -ge $TIMEOUT ]; then
  echo "FAIL: No non-profile Parquet flushes after ${TIMEOUT}s"
  echo "=== Collector logs ==="
  docker compose logs collector
  exit 1
fi

# Give a few seconds for all signal flushes to complete
echo "Waiting 10s for remaining flushes..."
sleep 10

echo "=== Running DuckDB verification ==="

# Determine DuckDB command: prefer local CLI, fall back to Docker
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
PARQUET="read_parquet('s3://sigiro/**/*.parquet')"

FAILURES=0
PASSES=0
SKIPS=0

run_check() {
  local name="$1"
  local query="$2"
  local expected="$3"
  printf "  %-45s" "$name..."
  result=$($DUCKDB -noheader -csv -c "${S3_SETUP} ${query}" 2>&1) || true
  if echo "$result" | grep -q "$expected"; then
    echo "PASS"
    PASSES=$((PASSES + 1))
  else
    echo "FAIL"
    echo "    Expected to match: $expected"
    echo "    Got: $result"
    FAILURES=$((FAILURES + 1))
  fi
}

echo ""
echo "=== Check 1: All 4 signal types present ==="
run_check "Signal: log" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='log';" \
  "true"
run_check "Signal: metric" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='metric';" \
  "true"
run_check "Signal: profile" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile';" \
  "true"
run_check "Signal: span" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span';" \
  "true"

echo ""
echo "=== Check 2: Both services present ==="
run_check "Service: checkout" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE service='checkout';" \
  "true"
run_check "Service: payments" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE service='payments';" \
  "true"

echo ""
echo "=== Check 3: Metrics aggregate correctly ==="
run_check "Metrics time series" \
  "SELECT count(*) > 0 FROM (
    SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M') AS bucket,
           name, avg(value) AS avg_val, count(*) AS n
    FROM $PARQUET
    WHERE signal='metric' AND service='checkout'
    GROUP BY bucket, name
    LIMIT 5
  );" \
  "true"

echo ""
echo "=== Check 4: Spans have trace_id ==="
run_check "Spans have trace_id" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND trace_id IS NOT NULL;" \
  "true"

echo ""
echo "=== Check 5: Spans have parent_span_id ==="
run_check "Spans have parent_span_id" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND parent_span_id IS NOT NULL;" \
  "true"

echo ""
echo "=== Check 6: Profiles have stacktrace ==="
run_check "Profiles have stacktrace" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" \
  "true"

echo ""
echo "=== Check 7: Profiles from both services ==="
run_check "Profiles: checkout" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='checkout';" \
  "true"
run_check "Profiles: payments" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='payments';" \
  "true"

echo ""
echo "=== Check 8: Investigation block — all signals for checkout ==="
run_check "Investigation: metrics" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='metric' AND service='checkout';" \
  "true"
run_check "Investigation: spans" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND service='checkout';" \
  "true"
run_check "Investigation: logs" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='log' AND service='checkout';" \
  "true"
run_check "Investigation: profiles" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='checkout';" \
  "true"

echo ""
echo "=== Check 9: Self-observability (collector's own telemetry) ==="
run_check "Collector metrics exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE service='sigiro-collector' AND signal='metric';" \
  "true"
run_check "Collector logs exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE service='sigiro-collector' AND signal='log';" \
  "true"
run_check "Collector traces exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE service='sigiro-collector' AND signal='span';" \
  "true"
run_check "Collector self-profiles exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE service='sigiro-collector' AND signal='profile';" \
  "true"
run_check "Collector otelcol_* metrics" \
  "SELECT count(DISTINCT name) >= 5 FROM $PARQUET WHERE service='sigiro-collector' AND signal='metric' AND name LIKE 'otelcol_%';" \
  "true"

# Cross-service traces: SKIP — telemetrygen generates independent trace_ids per
# service, so no shared traces exist. The exporter code path is identical either
# way. Cross-service linking is validated in the integration_test.go unit tests.
echo ""
echo "  SKIP: Cross-service traces (telemetrygen generates independent trace_ids per service)"
SKIPS=$((SKIPS + 1))

echo ""
echo "==============================="
echo "  Passed: $PASSES"
echo "  Failed: $FAILURES"
echo "  Skipped: $SKIPS"
echo "==============================="

if [ $FAILURES -gt 0 ]; then
  echo "=== SOME CHECKS FAILED ==="
  exit 1
fi

echo "=== ALL CHECKS PASSED ==="
