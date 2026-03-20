#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

export DOCKER_BUILDKIT=1

SIGIRO_URL="http://localhost:9999"

cleanup() {
  echo "=== Cleaning up ==="
  docker compose down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Building sigiro image ==="
docker compose build

echo "=== Starting sigiro ==="
docker compose up -d sigiro

echo "Waiting for sigiro to be healthy..."
TIMEOUT=60
ELAPSED=0
while [ $ELAPSED -lt $TIMEOUT ]; do
  if curl -sf "$SIGIRO_URL/health" >/dev/null 2>&1; then
    echo "sigiro healthy after ${ELAPSED}s"
    break
  fi
  sleep 2
  ELAPSED=$((ELAPSED + 2))
done
if [ $ELAPSED -ge $TIMEOUT ]; then
  echo "FAIL: sigiro not healthy after ${TIMEOUT}s"
  docker compose logs sigiro
  exit 1
fi

echo "=== Starting telemetry generators ==="
docker compose up -d \
  telemetrygen-checkout-traces \
  telemetrygen-checkout-metrics \
  telemetrygen-checkout-logs \
  telemetrygen-payments-traces \
  telemetrygen-payments-metrics \
  telemetrygen-payments-logs

echo "=== Waiting for telemetry to be ingested (35s) ==="
sleep 35

FAILURES=0
PASSES=0

query() {
  curl -sf -X POST "$SIGIRO_URL/query" -d "$1" 2>&1
}

run_check() {
  local name="$1"
  local sql="$2"
  local expected="$3"
  printf "  %-50s" "$name..."
  result=$(query "$sql") || true
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
echo "=== Check 1: All 3 signal types present ==="
run_check "Signal: spans" \
  "SELECT count(*) AS n FROM sigiro_spans" \
  '"n"'
run_check "Signal: logs" \
  "SELECT count(*) AS n FROM sigiro_logs" \
  '"n"'
run_check "Signal: metrics_gauge" \
  "SELECT count(*) AS n FROM sigiro_metrics_gauge" \
  '"n"'

echo ""
echo "=== Check 2: Both services present ==="
run_check "Service: checkout spans" \
  "SELECT count(*) > 0 AS ok FROM sigiro_spans WHERE service_name='checkout'" \
  "true"
run_check "Service: payments spans" \
  "SELECT count(*) > 0 AS ok FROM sigiro_spans WHERE service_name='payments'" \
  "true"

echo ""
echo "=== Check 3: Spans have trace_id and parent_span_id ==="
run_check "Spans have trace_id" \
  "SELECT count(*) > 0 AS ok FROM sigiro_spans WHERE trace_id IS NOT NULL AND trace_id != ''" \
  "true"
run_check "Spans have parent_span_id" \
  "SELECT count(*) > 0 AS ok FROM sigiro_spans WHERE parent_span_id IS NOT NULL AND parent_span_id != ''" \
  "true"

echo ""
echo "=== Check 4: Metrics aggregate correctly ==="
run_check "Metrics time series" \
  "SELECT count(*) > 0 AS ok FROM (SELECT time_bucket(INTERVAL '1 minute', timestamp) AS bucket, metric_name, avg(value) FROM sigiro_metrics_gauge WHERE service_name='checkout' GROUP BY bucket, metric_name LIMIT 5)" \
  "true"

echo ""
echo "=== Check 5: /health returns 200 ==="
printf "  %-50s" "/health returns ok..."
health=$(curl -sf "$SIGIRO_URL/health") || true
if [ "$health" = "ok" ]; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (got: $health)"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 6: /investigate returns valid JSON ==="
printf "  %-50s" "/investigate returns data..."
inv=$(curl -sf -X POST "$SIGIRO_URL/investigate" \
  -H 'Content-Type: application/json' \
  -d '{"service":"checkout"}') || true
if echo "$inv" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['service']=='checkout'" 2>/dev/null; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL"
  echo "    Got: ${inv:0:200}"
  FAILURES=$((FAILURES + 1))
fi

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
