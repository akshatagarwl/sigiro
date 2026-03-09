#!/usr/bin/env bash
# Verify Pyroscope SDK profiles landed in Parquet via DuckDB.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

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
PARQUET="read_parquet('s3://observe/**/*.parquet')"

FAILURES=0
PASSES=0

run_check() {
  local name="$1"
  local query="$2"
  local expected="$3"
  printf "  %-55s" "$name..."
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
echo "=== SDK Validation: Pyroscope rideshare profiles ==="

echo ""
echo "--- Check 1: Profile rows exist per service ---"
run_check "Go profiles exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.go';" \
  "true"
run_check "Python profiles exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.python';" \
  "true"
run_check "Node.js profiles exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.nodejs';" \
  "true"
run_check "Java profiles exist" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.java';" \
  "true"

echo ""
echo "--- Check 2: Stacktraces are non-empty ---"
run_check "Go stacktraces non-empty" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.go' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" \
  "true"
run_check "Python stacktraces non-empty" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.python' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" \
  "true"
run_check "Node.js stacktraces non-empty" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.nodejs' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" \
  "true"
run_check "Java stacktraces non-empty" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.java' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" \
  "true"

echo ""
echo "--- Check 3: Timestamps within last 10 minutes ---"
run_check "Go timestamps recent" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.go' AND timestamp > (extract(epoch from now()) - 600) * 1e9;" \
  "true"
run_check "Python timestamps recent" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.python' AND timestamp > (extract(epoch from now()) - 600) * 1e9;" \
  "true"
run_check "Node.js timestamps recent" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.nodejs' AND timestamp > (extract(epoch from now()) - 600) * 1e9;" \
  "true"
run_check "Java timestamps recent" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND service='rideshare.java' AND timestamp > (extract(epoch from now()) - 600) * 1e9;" \
  "true"

echo ""
echo "--- Check 4: Go app has multiple profile types ---"
run_check "Go has >= 2 profile types" \
  "SELECT count(DISTINCT name) >= 2 FROM $PARQUET WHERE signal='profile' AND service='rideshare.go';" \
  "true"

echo ""
echo "--- Check 5: Profile row counts ---"
run_check "Go profile count >= 5" \
  "SELECT count(*) >= 5 FROM $PARQUET WHERE signal='profile' AND service='rideshare.go';" \
  "true"
run_check "Python profile count >= 5" \
  "SELECT count(*) >= 5 FROM $PARQUET WHERE signal='profile' AND service='rideshare.python';" \
  "true"
run_check "Node.js profile count >= 1" \
  "SELECT count(*) >= 1 FROM $PARQUET WHERE signal='profile' AND service='rideshare.nodejs';" \
  "true"
run_check "Java profile count >= 5" \
  "SELECT count(*) >= 5 FROM $PARQUET WHERE signal='profile' AND service='rideshare.java';" \
  "true"

echo ""
echo "--- Check 6: Sample stacktrace contents ---"
$DUCKDB -noheader -c "${S3_SETUP}
SELECT service, name, len(stacktrace) AS stack_depth, stacktrace[1] AS top_frame
FROM $PARQUET
WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0
GROUP BY service, name, stacktrace
ORDER BY service
LIMIT 12;
" 2>&1 || true

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
