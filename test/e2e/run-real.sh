#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

export DOCKER_BUILDKIT=1

COMPOSE="docker compose -f docker-compose.real.yaml"

# Cleanup on exit
cleanup() {
  echo "=== Cleaning up ==="
  $COMPOSE down -v --remove-orphans 2>/dev/null || true
  rm -rf "$SCRIPT_DIR/postgres-init.sql"
}
trap cleanup EXIT

# Fetch postgres init SQL from upstream
echo "=== Fetching postgres init SQL ==="
curl -sfL "https://raw.githubusercontent.com/open-telemetry/opentelemetry-demo/2.2.0/src/postgresql/init.sql" \
  -o "$SCRIPT_DIR/postgres-init.sql"

echo "=== Building images ==="
$COMPOSE build

echo "=== Phase 1: Infrastructure ==="
$COMPOSE up -d minio minio-setup otel-collector flagd valkey-cart postgresql kafka
echo "Waiting for infrastructure to be ready..."
sleep 10

echo "=== Phase 2: App services ==="
$COMPOSE up -d \
  ad cart checkout currency email payment \
  product-catalog quote recommendation shipping \
  frontend image-provider
echo "Waiting for services to start..."
sleep 30

echo "=== Phase 3: Load generator ==="
$COMPOSE up -d load-generator

echo "=== Waiting for Parquet files ==="
TIMEOUT=180
ELAPSED=0
while [ $ELAPSED -lt $TIMEOUT ]; do
  FLUSH_COUNT=$($COMPOSE logs otel-collector 2>&1 | grep -c "flushed parquet" || true)
  if [ "$FLUSH_COUNT" -gt 0 ]; then
    echo "Found $FLUSH_COUNT flush(es) in collector logs after ${ELAPSED}s"
    break
  fi
  sleep 10
  ELAPSED=$((ELAPSED + 10))
done

if [ $ELAPSED -ge $TIMEOUT ]; then
  echo "FAIL: No Parquet flushes after ${TIMEOUT}s"
  echo "=== Collector logs ==="
  $COMPOSE logs otel-collector
  exit 1
fi

# Wait for load generator to produce cross-service traces
echo "Waiting 180s for cross-service telemetry..."
sleep 180

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

run_check() {
  local name="$1"
  local query="$2"
  local expected="$3"
  printf "  %-50s" "$name..."
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
echo "=== Check 1: Log, metric, span signals present ==="
run_check "Signal: log" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='log';" \
  "true"
run_check "Signal: metric" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='metric';" \
  "true"
run_check "Signal: span" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span';" \
  "true"

echo ""
echo "=== Check 2: >= 5 distinct services ==="
run_check "Distinct services >= 5" \
  "SELECT count(DISTINCT service) >= 5 FROM $PARQUET;" \
  "true"

echo ""
echo "=== Check 3: Cross-service traces exist ==="
run_check "Cross-service traces" \
  "SELECT count(*) > 0 FROM (
    SELECT trace_id, count(DISTINCT service) AS svc_count
    FROM $PARQUET
    WHERE signal='span' AND trace_id IS NOT NULL
    GROUP BY trace_id
    HAVING svc_count > 1
    LIMIT 1
  );" \
  "true"

echo ""
echo "=== Check 4: Span tree depth >= 3 ==="
run_check "Span tree depth >= 3" \
  "SELECT count(*) > 0 FROM (
    WITH spans AS (
      SELECT span_id, parent_span_id
      FROM $PARQUET
      WHERE signal='span' AND span_id IS NOT NULL
    )
    SELECT c.span_id
    FROM spans c
    JOIN spans p ON c.parent_span_id = p.span_id
    JOIN spans gp ON p.parent_span_id = gp.span_id
    LIMIT 1
  );" \
  "true"

echo ""
echo "=== Check 5: Spans have trace_id ==="
run_check "Spans have trace_id" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND trace_id IS NOT NULL;" \
  "true"

echo ""
echo "=== Check 6: Investigation block for checkout ==="
run_check "Investigation: metrics" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='metric' AND service='checkout';" \
  "true"
run_check "Investigation: spans" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND service='checkout';" \
  "true"
run_check "Investigation: logs" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='log' AND service='checkout';" \
  "true"

echo ""
echo "=== Check 7: Metrics have real names ==="
run_check "Distinct metric names >= 3" \
  "SELECT count(DISTINCT name) >= 3 FROM $PARQUET WHERE signal='metric';" \
  "true"

echo ""
echo "==============================="
echo "  Passed: $PASSES"
echo "  Failed: $FAILURES"
echo "==============================="

if [ $FAILURES -gt 0 ]; then
  echo "=== SOME CHECKS FAILED ==="
  exit 1
fi

echo "=== ALL CHECKS PASSED ==="
