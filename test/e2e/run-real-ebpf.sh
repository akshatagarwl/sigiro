#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

export DOCKER_BUILDKIT=1

OVERRIDE_EBPF="/tmp/e2e-ebpf.override.yaml"

compose() {
  docker compose -f docker-compose.real.yaml -f "$OVERRIDE_EBPF" "$@"
}

cleanup() {
  echo "=== Cleaning up ==="
  compose down -v --remove-orphans 2>/dev/null || true
  rm -f "$SCRIPT_DIR/postgres-init.sql" "$OVERRIDE_EBPF"
}
trap cleanup EXIT

echo "=== Fetching postgres init SQL ==="
rm -rf "$SCRIPT_DIR/postgres-init.sql"
curl -sfL "https://raw.githubusercontent.com/open-telemetry/opentelemetry-demo/2.2.0/src/postgresql/init.sql" \
  -o "$SCRIPT_DIR/postgres-init.sql"

echo "=== Writing compose overrides ==="
cat > "$OVERRIDE_EBPF" <<'YAML'
services:
  otel-collector:
    command: ["--config", "/etc/otelcol/otelcol.yaml", "--feature-gates=service.profilesSupport"]
    volumes:
      - ../../collector/otelcol.yaml:/etc/otelcol/otelcol.yaml:ro
      - /sys/kernel/debug:/sys/kernel/debug
      - /sys/kernel/tracing:/sys/kernel/tracing
    privileged: true
    pid: host
  profilegen:
    profiles: ["disabled"]
YAML

echo "=== Ensuring host tracefs/debugfs mounts ==="
docker run --rm --privileged alpine sh -lc 'mount -t tracefs tracefs /sys/kernel/tracing || true; mount -t debugfs debugfs /sys/kernel/debug || true'

echo "=== Building images ==="
compose build otel-collector

echo "=== Phase 1: Infrastructure ==="
compose up -d minio minio-setup otel-collector flagd valkey-cart postgresql
echo "Waiting for infrastructure..."
sleep 10

echo "=== Phase 2: App services (no profilegen) ==="
compose up -d \
  ad cart checkout currency email payment \
  product-catalog quote recommendation shipping \
  frontend image-provider
sleep 30

echo "=== Phase 3: Load generator ==="
compose up -d load-generator

echo "=== Waiting for telemetry collection ==="
sleep 180

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

FAILURES=0
PASSES=0

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
echo "=== Signal checks ==="
run_check "Signal: log" "SELECT count(*) > 0 FROM $PARQUET WHERE signal='log';" "true"
run_check "Signal: metric" "SELECT count(*) > 0 FROM $PARQUET WHERE signal='metric';" "true"
run_check "Signal: profile" "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile';" "true"
run_check "Signal: span" "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span';" "true"

echo ""
echo "=== Service + trace checks ==="
run_check "Distinct services >= 5" "SELECT count(DISTINCT service) >= 5 FROM $PARQUET WHERE service IS NOT NULL AND service <> '';" "true"
run_check "Cross-service traces" "SELECT count(*) > 0 FROM (SELECT trace_id, count(DISTINCT service) AS svc_count FROM $PARQUET WHERE signal='span' AND trace_id IS NOT NULL GROUP BY trace_id HAVING svc_count > 1 LIMIT 1);" "true"

echo ""
echo "=== Collector crash check ==="
if compose logs otel-collector --tail=500 | grep -q "panic:"; then
  echo "  Collector panic absent...FAIL"
  FAILURES=$((FAILURES + 1))
else
  echo "  Collector panic absent...PASS"
  PASSES=$((PASSES + 1))
fi

echo ""
echo "==============================="
echo "  Passed: $PASSES"
echo "  Failed: $FAILURES"
echo "==============================="

if [ $FAILURES -gt 0 ]; then
  echo "=== SOME CHECKS FAILED ==="
  echo "--- collector logs tail ---"
  compose logs otel-collector --tail=120 || true
  exit 1
fi

echo "=== ALL CHECKS PASSED ==="
