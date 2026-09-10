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

echo "=== Starting sigiro (self-profiling enabled) ==="
docker compose up -d sigiro

echo "Waiting for sigiro to be healthy..."
# Cold start budget (image WITHOUT pre-warmed extensions): DuckDB extension
# download (30-90s per extension × 3) + DuckLake init + init_complete flip.
# Cold start budget (image WITH pre-warmed extensions): <5s.
# 300s comfortably covers either case; subsequent runs are typically <5s.
TIMEOUT=300
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

FAILURES=0
PASSES=0
DEV_AUTH="Authorization: Bearer sigiro_dev_e2e_test"

echo "=== Waiting for DuckLake init + profiling data to accumulate ==="
echo "  (waiting for both CPU and heap agents to complete a flush)"
DATA_WAIT=0
DATA_TIMEOUT=180
while [ $DATA_WAIT -lt $DATA_TIMEOUT ]; do
  cpu_count=$(curl -sf -X POST "$SIGIRO_URL/v1/query" \
    -H "$DEV_AUTH" \
    -d "SELECT count(*) AS n FROM lake.sigiro_profiles WHERE profile_type = 'cpu'" \
    2>/dev/null || echo "")
  heap_count=$(curl -sf -X POST "$SIGIRO_URL/v1/query" \
    -H "$DEV_AUTH" \
    -d "SELECT count(*) AS n FROM lake.sigiro_profiles WHERE profile_type IN ('alloc_objects', 'alloc_space', 'inuse_objects', 'inuse_space')" \
    2>/dev/null || echo "")
  cpu_count=$(echo "$cpu_count" | jq -r '.[0].n // 0' 2>/dev/null || echo 0)
  heap_count=$(echo "$heap_count" | jq -r '.[0].n // 0' 2>/dev/null || echo 0)
  if [ "$cpu_count" -gt 0 ] 2>/dev/null && [ "$heap_count" -gt 0 ] 2>/dev/null; then
    echo "  CPU and heap profiles available after ${DATA_WAIT}s"
    break
  fi
  sleep 5
  DATA_WAIT=$((DATA_WAIT + 5))
  if [ $((DATA_WAIT % 30)) -eq 0 ]; then
    echo "  still waiting... (${DATA_WAIT}s elapsed)"
  fi
done
if [ $DATA_WAIT -ge $DATA_TIMEOUT ]; then
  echo "FAIL: CPU and heap profiles not both available after ${DATA_TIMEOUT}s"
  docker compose logs sigiro
  exit 1
fi

query() {
  curl -sf -X POST "$SIGIRO_URL/v1/query" -H "$DEV_AUTH" -d "$1" 2>&1
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
echo "=== Check 1: /health returns 200 with status:ready ==="
printf "  %-50s" "/health returns 200 with status:ready..."
health_code=$(curl -s -o /tmp/health_body -w "%{http_code}" "$SIGIRO_URL/health") || true
health_body=$(cat /tmp/health_body 2>/dev/null || echo "")
if [ "$health_code" = "200" ] && echo "$health_body" | grep -q '"status":"ready"'; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (code=$health_code body=$health_body)"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 2: Open mode accepts requests without a key ==="
no_auth_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SIGIRO_URL/v1/query" \
  -d "SELECT 1 AS ok") || true
printf "  %-50s" "no auth header returns 200..."
if [ "$no_auth_code" = "200" ]; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (got: $no_auth_code)"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 3: Self-profiling (SIGIRO_PYROSCOPE_ENDPOINT) ==="
# DuckLake schema is "main" for local DuckDB-file catalogs and "lake" for
# PostgreSQL-backed federation catalogs. Detect which is in use and run
# against that schema so the test works in both deployments.
PROFILE_SCHEMA=$(curl -sf -X POST "$SIGIRO_URL/v1/query" \
  -H "$DEV_AUTH" \
  -d "SELECT table_schema FROM information_schema.tables WHERE table_name = 'sigiro_profiles' LIMIT 1" 2>/dev/null \
  | jq -r '.[0].table_schema // "main"' 2>/dev/null)
PROFILE_SCHEMA="${PROFILE_SCHEMA:-main}"
echo "  detected profile schema: $PROFILE_SCHEMA"
run_check "Sigiro self-profiles count > 0" \
  "SELECT count(*) AS n FROM $PROFILE_SCHEMA.sigiro_profiles" \
  '"n"'
run_check "Sigiro self-profile has stack_trace" \
  "SELECT count(*) > 0 AS ok FROM $PROFILE_SCHEMA.sigiro_profiles WHERE stack_trace IS NOT NULL AND array_length(stack_trace) > 0" \
  "true"
run_check "Sigiro self-profile has service_name" \
  "SELECT count(*) > 0 AS ok FROM $PROFILE_SCHEMA.sigiro_profiles WHERE service_name = 'sigiro'" \
  "true"
run_check "Sigiro self-profile has CPU samples" \
  "SELECT count(*) > 0 AS ok FROM $PROFILE_SCHEMA.sigiro_profiles WHERE profile_type = 'cpu'" \
  "true"
run_check "Sigiro self-profile has heap samples" \
  "SELECT count(*) > 0 AS ok FROM $PROFILE_SCHEMA.sigiro_profiles WHERE profile_type IN ('alloc_objects', 'alloc_space', 'inuse_objects', 'inuse_space')" \
  "true"

echo ""
echo "=== Check 4: /v1/diagnose profiles ==="
# Diagnose requires from_ts and to_ts (Unix microseconds). Use a wide
# window (last 1 hour) so any recent self-profile is included.
NOW_US=$(($(date +%s) * 1000000))
HOUR_AGO_US=$((NOW_US - 3600 * 1000000))
inv=$(curl -sf -X POST "$SIGIRO_URL/v1/diagnose" \
  -H "$DEV_AUTH" \
  -H 'Content-Type: application/json' \
  -d "{\"services\":[\"sigiro\"],\"from_ts\":${HOUR_AGO_US},\"to_ts\":${NOW_US}}") || true
printf "  %-50s" "diagnose returns CPU and heap profiles..."
if echo "$inv" | jq -e '
  .results.sigiro.profiles |
  (map(select(.profile_type == "cpu")) | length > 0) and
  (map(select(.profile_type == "alloc_objects" or .profile_type == "alloc_space" or .profile_type == "inuse_objects" or .profile_type == "inuse_space")) | length > 0)
' >/dev/null 2>&1; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL"
  echo "    Got: ${inv:0:300}"
  FAILURES=$((FAILURES + 1))
fi

latest_heap_ts=$(query \
  "SELECT max(timestamp) AS ts FROM $PROFILE_SCHEMA.sigiro_profiles WHERE profile_type = 'inuse_space'" \
  | jq -r '.[0].ts')
latest_heap_total=$(query \
  "SELECT sum(value) AS total FROM $PROFILE_SCHEMA.sigiro_profiles WHERE profile_type = 'inuse_space' AND timestamp = make_timestamp(${latest_heap_ts})" \
  | jq -r '.[0].total')
reported_heap_total=$(echo "$inv" | jq -r \
  '[.results.sigiro.profiles[] | select(.profile_type == "inuse_space") | .total_value] | add // 0')
printf "  %-50s" "diagnose reports only the latest heap snapshot..."
if [ "$reported_heap_total" -le "$latest_heap_total" ] 2>/dev/null; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (reported=$reported_heap_total latest-snapshot=$latest_heap_total)"
  FAILURES=$((FAILURES + 1))
fi

printf "  %-50s" "profile stacks use DuckDB VARCHAR[] / JSON arrays..."
latest_heap_stack=$(query \
  "SELECT stack_trace FROM $PROFILE_SCHEMA.sigiro_profiles WHERE profile_type = 'inuse_space' AND timestamp = make_timestamp(${latest_heap_ts}) LIMIT 1")
if echo "$latest_heap_stack" | jq -e '.[0].stack_trace | type == "array"' >/dev/null 2>&1; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL"
  echo "    stack_trace was not serialized as a JSON array"
  FAILURES=$((FAILURES + 1))
fi

printf "  %-50s" "heap allocation frames are symbolized..."
heap_frames=$(query \
  "SELECT unnest(stack_trace) AS frame FROM $PROFILE_SCHEMA.sigiro_profiles WHERE profile_type = 'inuse_space' AND timestamp = make_timestamp(${latest_heap_ts}) LIMIT 10000")
if echo "$heap_frames" | jq -e \
  'any(.[]; .frame | contains("DuckLake"))' \
  >/dev/null 2>&1; then
  echo "PASS"
  PASSES=$((PASSES + 1))
else
  echo "FAIL"
  echo "    Heap profile contains no resolved DuckLake extension frame"
  FAILURES=$((FAILURES + 1))
fi

echo ""
echo "=== Check 5: Pyroscope self-send to :4318 ==="
# Use grep -c with || true so a "no matches" result (exit 1) doesn't trigger
# the fallback echo, which produces "0\n0" and confuses the integer compare.
sigiro_logs=$(docker compose logs sigiro 2>&1 | { grep -c "pyroscope ingest" || true; })
printf "  %-50s" "sigiro received pyroscope ingest logs..."
if [ "${sigiro_logs:-0}" -gt "0" ] 2>/dev/null; then
  echo "PASS (found $sigiro_logs ingest log lines)"
  PASSES=$((PASSES + 1))
else
  echo "FAIL (no pyroscope ingest logs found)"
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
