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
# download (30-90s per extension × 3) + DuckLake init + .ready write.
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
echo "  (pyroscope SDK needs ~90s to send first real profile batch)"
DATA_WAIT=0
DATA_TIMEOUT=180
while [ $DATA_WAIT -lt $DATA_TIMEOUT ]; do
  profile_count=$(curl -sf -X POST "$SIGIRO_URL/v1/query" \
    -H "$DEV_AUTH" -d "SELECT count(*) AS n FROM lake.sigiro_profiles" 2>/dev/null || echo "")
  if echo "$profile_count" | grep -q '"n"' 2>/dev/null; then
    count_val=$(echo "$profile_count" | jq -r '.[0].n // 0' 2>/dev/null || echo "0")
    if [ "$count_val" -gt "0" ] 2>/dev/null; then
      echo "  profiles available after ${DATA_WAIT}s (${count_val} rows)"
      break
    fi
  fi
  sleep 5
  DATA_WAIT=$((DATA_WAIT + 5))
  if [ $((DATA_WAIT % 30)) -eq 0 ]; then
    echo "  still waiting... (${DATA_WAIT}s elapsed)"
  fi
done
if [ $DATA_WAIT -ge $DATA_TIMEOUT ]; then
  echo "  WARN: no profiles after ${DATA_TIMEOUT}s, continuing anyway"
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
echo "=== Check 2: Auth — sigiro_dev_ key accepted ==="
auth_result=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "$SIGIRO_URL/v1/query" \
  -H "$DEV_AUTH" \
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
echo "=== Check 3: Auth — no key returns 401 ==="
no_auth_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SIGIRO_URL/v1/query" \
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
echo "=== Check 4: Self-profiling (SIGIRO_PYROSCOPE_ENDPOINT) ==="
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

echo ""
echo "=== Check 5: /v1/investigate profiles ==="
# Investigate requires from_ts and to_ts (Unix microseconds). Use a wide
# window (last 1 hour) so any recent self-profile is included.
NOW_US=$(($(date +%s) * 1000000))
HOUR_AGO_US=$((NOW_US - 3600 * 1000000))
inv=$(curl -sf -X POST "$SIGIRO_URL/v1/investigate" \
  -H "$DEV_AUTH" \
  -H 'Content-Type: application/json' \
  -d "{\"service\":\"sigiro\",\"from_ts\":${HOUR_AGO_US},\"to_ts\":${NOW_US}}") || true
assert_contains "$inv" '"profiles"' "investigate: profiles section present"

echo ""
echo "=== Check 6: Pyroscope self-send to :4318 ==="
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
echo "=== DEBUG: DuckLake files directory ==="
ducklake_files=$(docker compose exec -T sigiro ls -la /var/lib/sigiro/dev_project/catalog.sqlite.files/ 2>&1 || echo "directory check failed")
echo "  DuckLake files dir: $ducklake_files"
echo "  (Informational only — DuckLake uses in-memory coordinator + Parquet files)"

echo ""
echo "=== DEBUG: Pyroscope profiles confirmed via /v1/investigate ==="
profile_check=$(curl -sf -X POST "$SIGIRO_URL/v1/investigate" \
  -H "$DEV_AUTH" \
  -H 'Content-Type: application/json' \
  -d "{\"service\":\"sigiro\",\"from_ts\":${HOUR_AGO_US},\"to_ts\":${NOW_US}}" 2>&1 | jq -r '
    .sigiro.profiles // [] |
    "profile_count=\(length)",
    "has_cpu_profile=\(map(select(.profile_type == "cpu")) | length > 0)"
  ' 2>/dev/null | paste -sd ',' - || echo "profile_check=failed")
echo "  $profile_check"

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
