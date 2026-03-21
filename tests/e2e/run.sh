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
  telemetrygen-payments-logs \
  telemetrygen-http-traces \
  telemetrygen-http-logs

echo "=== Waiting for telemetry to be ingested (35s) ==="
sleep 35

FAILURES=0
PASSES=0

DEV_AUTH="Authorization: Bearer sk_dev_e2e_test"

query() {
  curl -sf -X POST "$SIGIRO_URL/query" -H "$DEV_AUTH" -d "$1" 2>&1
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

# assert_contains: check that $1 contains substring $2
assert_contains() {
  local haystack="$1"
  local needle="$2"
  local name="$3"
  printf "  %-50s" "$name..."
  if echo "$haystack" | grep -q "$needle"; then
    echo "PASS"
    PASSES=$((PASSES + 1))
  else
    echo "FAIL"
    echo "    Expected to contain: $needle"
    echo "    Got: ${haystack:0:300}"
    FAILURES=$((FAILURES + 1))
  fi
}

# assert_json: check a jq expression returns expected value
assert_json() {
  local json="$1"
  local jq_expr="$2"
  local expected="$3"
  local name="$4"
  printf "  %-50s" "$name..."
  actual=$(echo "$json" | python3 -c "
import sys, json
d = json.load(sys.stdin)
path = '''$jq_expr'''
# Navigate dotted path
obj = d
for key in path.strip('.').split('.'):
    if isinstance(obj, dict):
        obj = obj.get(key)
    else:
        obj = None
        break
print(json.dumps(obj))
" 2>/dev/null) || actual="null"
  if [ "$actual" = "$expected" ]; then
    echo "PASS"
    PASSES=$((PASSES + 1))
  else
    echo "FAIL"
    echo "    Path: $jq_expr"
    echo "    Expected: $expected"
    echo "    Got: $actual"
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
inv=$(curl -sf -X POST "$SIGIRO_URL/investigate" \
  -H "$DEV_AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"service":"checkout"}') || true
assert_contains "$inv" '"service":"checkout"' "/investigate returns checkout"

echo ""
echo "=== Check 7: /query endpoint via HTTP ==="
query_result=$(curl -sf -X POST "$SIGIRO_URL/query" \
  -H "$DEV_AUTH" \
  -d "SELECT service_name, count(*) as cnt FROM sigiro_spans GROUP BY service_name") || true
assert_contains "$query_result" "checkout" "query: SELECT returns checkout"
assert_contains "$query_result" "payments" "query: SELECT returns payments"

echo ""
echo "=== Check 8: SQL injection blocked over HTTP ==="
drop_result=$(curl -s -X POST "$SIGIRO_URL/query" -H "Authorization: Bearer sk_dev_test" -d "DROP TABLE sigiro_spans") || true
assert_contains "$drop_result" "rejected" "blocklist: DROP rejected over HTTP"

echo ""
echo "=== Check 11: Auth — sk_dev_ key accepted ==="
auth_result=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "$SIGIRO_URL/query" \
  -H "Authorization: Bearer sk_dev_test" \
  -d "SELECT 1 AS ok") || true
printf "  %-50s" "sk_dev_ key returns 200..."
if [ "$auth_result" = "200" ]; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (got: $auth_result)"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 12: Auth — no key returns 401 ==="
no_auth_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SIGIRO_URL/query" \
  -d "SELECT 1 AS ok") || true
printf "  %-50s" "no auth header returns 401..."
if [ "$no_auth_code" = "401" ]; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (got: $no_auth_code)"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 13: Auth — invalid prefix returns 401 or 503 ==="
bad_key_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SIGIRO_URL/query" \
  -H "Authorization: Bearer sk_fake_bad_key" \
  -d "SELECT 1 AS ok") || true
printf "  %-50s" "invalid key returns 401/503..."
if [ "$bad_key_code" = "401" ] || [ "$bad_key_code" = "503" ]; then
  echo "PASS (got: $bad_key_code)"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (got: $bad_key_code)"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 9: /investigate/multi over HTTP ==="
multi_result=$(curl -sf -X POST "$SIGIRO_URL/investigate/multi" \
  -H "$DEV_AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"services":["checkout","payments"]}') || true
assert_contains "$multi_result" '"checkout"' "multi: checkout present"
assert_contains "$multi_result" '"payments"' "multi: payments present"

echo ""
echo "=== Check 10: HTTP OTLP ingest (:4318) ==="
http_svc_result=$(curl -sf -X POST "$SIGIRO_URL/investigate" \
  -H "$DEV_AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"service":"http-svc"}') || true
assert_json "$http_svc_result" ".coverage.has_spans" "true" "HTTP OTLP: http-svc has spans"
assert_json "$http_svc_result" ".coverage.has_logs" "true" "HTTP OTLP: http-svc has logs"

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
