#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

export DOCKER_BUILDKIT=1

OVERRIDE_EBPF="/tmp/e2e-ebpf.override.yaml"
OVERRIDE_QUERY="/tmp/e2e-real-query.override.yaml"

compose() {
  docker compose -f docker-compose.real.yaml -f "$OVERRIDE_EBPF" -f "$OVERRIDE_QUERY" "$@"
}

cleanup() {
  echo "=== Cleaning up ==="
  compose down -v --remove-orphans 2>/dev/null || true
  rm -f "$SCRIPT_DIR/postgres-init.sql" "$OVERRIDE_EBPF" "$OVERRIDE_QUERY"
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

cat > "$OVERRIDE_QUERY" <<'YAML'
services:
  query-server:
    build:
      context: ../..
      dockerfile: cmd/query-server/Dockerfile
    depends_on:
      otel-collector: { condition: service_started }
    environment:
      S3_ENDPOINT: minio:9000
      S3_BUCKET: observe
      S3_REGION: us-east-1
      S3_ACCESS_KEY: minioadmin
      S3_SECRET_KEY: minioadmin
      LISTEN_ADDR: ":8080"
    ports:
      - "8080:8080"
YAML

echo "=== Ensuring host tracefs/debugfs mounts ==="
docker run --rm --privileged alpine sh -lc 'mount -t tracefs tracefs /sys/kernel/tracing || true; mount -t debugfs debugfs /sys/kernel/debug || true'

echo "=== Building images ==="
compose build otel-collector query-server

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

echo "=== Starting query-server ==="
compose up -d query-server
sleep 8

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

SERVICE=$($DUCKDB -noheader -csv -c "${S3_SETUP}
  SELECT service FROM $PARQUET
  WHERE service = 'checkout' AND timestamp > 0
  LIMIT 1;" )
if [ -z "${SERVICE:-}" ]; then
  SERVICE=$($DUCKDB -noheader -csv -c "${S3_SETUP}
  SELECT service FROM $PARQUET
  WHERE timestamp > 0 AND service IS NOT NULL AND service <> ''
  ORDER BY timestamp DESC
  LIMIT 1;" )
fi
FROM_EPOCH=$($DUCKDB -noheader -csv -c "${S3_SETUP}
  SELECT CAST(floor(floor(max(timestamp)/1e9)/3600)*3600 AS BIGINT)
  FROM $PARQUET
  WHERE timestamp > 0 AND service = '$SERVICE';" )
TO_EPOCH=$((FROM_EPOCH + 3599))
FROM=$(date -u -r "$FROM_EPOCH" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || python - <<PY
import datetime
print(datetime.datetime.utcfromtimestamp($FROM_EPOCH).strftime('%Y-%m-%dT%H:%M:%SZ'))
PY
)
TO=$(date -u -r "$TO_EPOCH" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || python - <<PY
import datetime
print(datetime.datetime.utcfromtimestamp($TO_EPOCH).strftime('%Y-%m-%dT%H:%M:%SZ'))
PY
)

echo ""
echo "=== Query-server check ==="
echo "service=$SERVICE from=$FROM to=$TO"
HTTP_CODE=$(curl -sS -o /tmp/e2e-query-response.json -w "%{http_code}" \
  http://localhost:8080/v1/query \
  -d "{\"service\":\"$SERVICE\",\"from\":\"$FROM\",\"to\":\"$TO\"}" || true)

if [ "$HTTP_CODE" = "200" ]; then
  echo "  Query API HTTP 200...PASS"
  PASSES=$((PASSES + 1))
  if command -v jq &>/dev/null; then
    TOTAL_ROWS=$(jq -r '(.metrics|length)+(.spans|length)+(.logs|length)+(.profiles|length)' /tmp/e2e-query-response.json 2>/dev/null || echo "0")
  else
    TOTAL_ROWS=$(python3 - <<'PY'
import json
try:
    with open('/tmp/e2e-query-response.json', 'r', encoding='utf-8') as f:
        d = json.load(f)
    print(len(d.get('metrics', [])) + len(d.get('spans', [])) + len(d.get('logs', [])) + len(d.get('profiles', [])))
except Exception:
    print(0)
PY
)
  fi
  if [ "${TOTAL_ROWS:-0}" -gt 0 ]; then
    echo "  Query API returned data...PASS"
    PASSES=$((PASSES + 1))
  else
    echo "  Query API returned data...FAIL"
    FAILURES=$((FAILURES + 1))
  fi
  if command -v jq &>/dev/null; then
    jq '.service, (.metrics|length), (.spans|length), (.logs|length), (.profiles|length)' /tmp/e2e-query-response.json || true
  else
    head -c 300 /tmp/e2e-query-response.json
    echo
  fi
else
  echo "  Query API HTTP 200...FAIL"
  echo "    status=$HTTP_CODE body=$(head -c 220 /tmp/e2e-query-response.json 2>/dev/null || true)"
  FAILURES=$((FAILURES + 1))
fi

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
  echo "--- query-server logs tail ---"
  compose logs query-server --tail=120 || true
  exit 1
fi

echo "=== ALL CHECKS PASSED ==="
