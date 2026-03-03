#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

COMPOSE="docker compose -f docker-compose.yaml"

if command -v duckdb &>/dev/null; then
  DUCKDB="duckdb"
else
  DUCKDB="docker run --rm --network host duckdb/duckdb duckdb"
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

echo "=== Ensuring query stack is up ==="
$COMPOSE up -d minio minio-setup collector query-server >/dev/null

echo "=== Discovering real data window ==="
PARTITION_EPOCH=$($DUCKDB -noheader -csv -c "${S3_SETUP}
SELECT DISTINCT CAST(regexp_extract(filename, 's3://observe/([0-9]+)/', 1) AS BIGINT)
FROM read_parquet('s3://observe/**/*.parquet', filename=true)
LIMIT 1;" 2>/dev/null | head -n1 || true)

if [ -z "${PARTITION_EPOCH:-}" ]; then
  echo "No parquet data yet. Starting generators..."
  $COMPOSE up -d \
    telemetrygen-checkout-traces telemetrygen-checkout-metrics telemetrygen-checkout-logs \
    telemetrygen-payments-traces telemetrygen-payments-metrics telemetrygen-payments-logs \
    profilegen >/dev/null
  TIMEOUT=120
  ELAPSED=0
  while [ $ELAPSED -lt $TIMEOUT ]; do
    PARTITION_EPOCH=$($DUCKDB -noheader -csv -c "${S3_SETUP}
SELECT DISTINCT CAST(regexp_extract(filename, 's3://observe/([0-9]+)/', 1) AS BIGINT)
FROM read_parquet('s3://observe/**/*.parquet', filename=true)
LIMIT 1;" 2>/dev/null | head -n1 || true)
    if [ -n "${PARTITION_EPOCH:-}" ]; then
      break
    fi
    sleep 5
    ELAPSED=$((ELAPSED + 5))
  done
  if [ -z "${PARTITION_EPOCH:-}" ]; then
    echo "Still no parquet data found in s3://observe/**/*.parquet"
    echo "--- collector logs (tail 80) ---"
    $COMPOSE logs collector --tail=80 || true
    exit 1
  fi
fi

SERVICE=$($DUCKDB -noheader -csv -c "${S3_SETUP}
SELECT service FROM $PARQUET
WHERE service IS NOT NULL AND service <> '' AND timestamp > 0
ORDER BY timestamp DESC
LIMIT 1;")
SERVICE_EPOCH=$($DUCKDB -noheader -csv -c "${S3_SETUP}
SELECT CAST(floor(max(timestamp)/1e9) AS BIGINT)
FROM $PARQUET
WHERE service = '$SERVICE' AND timestamp > 0;")
FROM=$(date -u -r "$((SERVICE_EPOCH - SERVICE_EPOCH % 3600))" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || python - <<PY
import datetime
ts=$SERVICE_EPOCH - ($SERVICE_EPOCH % 3600)
print(datetime.datetime.utcfromtimestamp(ts).strftime('%Y-%m-%dT%H:%M:%SZ'))
PY
)
TO=$(date -u -r "$((SERVICE_EPOCH - SERVICE_EPOCH % 3600 + 3599))" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || python - <<PY
import datetime
ts=$SERVICE_EPOCH - ($SERVICE_EPOCH % 3600) + 3599
print(datetime.datetime.utcfromtimestamp(ts).strftime('%Y-%m-%dT%H:%M:%SZ'))
PY
)
TRACE_ID=$($DUCKDB -noheader -csv -c "${S3_SETUP}
SELECT trace_id FROM $PARQUET
WHERE signal='span' AND trace_id IS NOT NULL AND timestamp > 0
ORDER BY timestamp DESC
LIMIT 1;")

echo "partition=$PARTITION_EPOCH service=$SERVICE from=$FROM to=$TO"

echo ""
echo "=== Query by service window ==="
SERVICE_RESP=$(curl -sS -X POST http://localhost:8080/v1/query \
  -d "{\"service\":\"$SERVICE\",\"from\":\"$FROM\",\"to\":\"$TO\"}")
if command -v jq &>/dev/null; then
  echo "$SERVICE_RESP" | jq .
else
  echo "$SERVICE_RESP"
fi

if [ -n "${TRACE_ID:-}" ]; then
  echo ""
  echo "=== Query by trace_id ==="
  TRACE_RESP=$(curl -sS -X POST http://localhost:8080/v1/query \
    -d "{\"trace_id\":\"$TRACE_ID\"}")
  if command -v jq &>/dev/null; then
    echo "$TRACE_RESP" | jq .
  else
    echo "$TRACE_RESP"
  fi
fi
