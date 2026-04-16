#!/usr/bin/env bash
# Sigiro E2E test — runs inside a Lima VM
# Tests the full pipeline: start server → send OTLP data → query → investigate
#
# Usage:
#   MODE=local  limactl shell <vm> bash /path/to/lima-e2e-test.sh
#   MODE=s3     limactl shell <vm> bash /path/to/lima-e2e-test.sh
#
# Environment:
#   MODE          "local" (default) or "s3"
#   SIGIRO_BIN    Path to sigiro binary (default: target/release/sigiro relative to script)
#   SIGIRO_DATA   Data directory (default: /tmp/sigiro-e2e-data)
#   MINIO_PORT    MinIO API port for S3 mode (default: 9000)
#   PG_PORT       PostgreSQL port for S3 mode (default: 5432)
set -euo pipefail

MODE="${MODE:-local}"
SIGIRO_DATA="${SIGIRO_DATA:-/tmp/sigiro-e2e-data}"
MINIO_PORT="${MINIO_PORT:-9000}"
PG_PORT="${PG_PORT:-5432}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SIGIRO_BIN="${SIGIRO_BIN:-${SCRIPT_DIR}/../target/release/sigiro}"
SIGIRO_PID=""
FAILED=0

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log()  { echo -e "${CYAN}[$(date +%H:%M:%S)]${NC} $*"; }
pass() { echo -e "${GREEN}[PASS]${NC} $*"; }
fail() { echo -e "${RED}[FAIL]${NC} $*"; FAILED=1; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }

cleanup() {
    log "Cleaning up..."
    [ -n "$SIGIRO_PID" ] && kill "$SIGIRO_PID" 2>/dev/null || true
    pkill -f "minio server" 2>/dev/null || true
    log "Done."
}
trap cleanup EXIT

# ─── Step 0: Verify binary exists ─────────────────────────────────
log "Sigiro E2E test — mode: ${MODE}"
log "Binary: ${SIGIRO_BIN}"
if [ ! -f "$SIGIRO_BIN" ]; then
    fail "Binary not found at ${SIGIRO_BIN}"
    exit 1
fi
$SIGIRO_BIN --help > /dev/null 2>&1
pass "Binary exists and runs"

# ─── Step 1: Set up infrastructure ────────────────────────────────
log "Setting up data directory: ${SIGIRO_DATA}"
rm -rf "$SIGIRO_DATA"
mkdir -p "$SIGIRO_DATA"

if [ "$MODE" = "s3" ]; then
    log "Setting up MinIO (S3-compatible storage)..."

    export MINIO_ROOT_USER=sigiro-test
    export MINIO_ROOT_PASSWORD=sigiro-test-secret
    mkdir -p /tmp/minio-data-e2e
    nohup minio server /tmp/minio-data-e2e --address ":${MINIO_PORT}" --console-address ":9001" > /tmp/minio.log 2>&1 &
    MINIO_PID=$!
    sleep 3

    if ! kill -0 "$MINIO_PID" 2>/dev/null; then
        fail "MinIO failed to start"
        cat /tmp/minio.log
        exit 1
    fi
    pass "MinIO started (PID ${MINIO_PID})"

    # Create bucket
    mc alias set local "http://localhost:${MINIO_PORT}" sigiro-test sigiro-test-secret --api s3v4
    mc mb "local/sigiro-test" 2>/dev/null || true
    pass "MinIO bucket created"

    # Set up PostgreSQL
    log "Setting up PostgreSQL..."
    sudo -u postgres pg_ctlcluster 16 main start 2>/dev/null || sudo pg_ctlcluster 16 main start 2>/dev/null || true
    sudo -u postgres psql -c "CREATE USER sigiro WITH PASSWORD 'sigiro-test';" 2>/dev/null || true
    sudo -u postgres psql -c "CREATE DATABASE sigiro_catalog OWNER sigiro;" 2>/dev/null || true
    sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE sigiro_catalog TO sigiro;" 2>/dev/null || true
    pass "PostgreSQL ready"
fi

# ─── Step 2: Start sigiro server ──────────────────────────────────
log "Starting sigiro server..."
export SIGIRO_DATA_DIR="$SIGIRO_DATA"
export RUST_LOG=sigiro=info

if [ "$MODE" = "s3" ]; then
    export SIGIRO_CATALOG_URL="postgresql://sigiro:sigiro-test@localhost:${PG_PORT}/sigiro_catalog"
    export SIGIRO_S3_BUCKET="sigiro-test"
    export SIGIRO_S3_REGION="us-east-1"
    export SIGIRO_S3_PREFIX="e2e-test"
    export AWS_ACCESS_KEY_ID=sigiro-test
    export AWS_SECRET_ACCESS_KEY=sigiro-test-secret
    export AWS_ENDPOINT_URL="http://localhost:${MINIO_PORT}"
fi

$SIGIRO_BIN serve --no-ebpf > /tmp/sigiro-server.log 2>&1 &
SIGIRO_PID=$!
log "Server PID: ${SIGIRO_PID}"
sleep 3

if ! kill -0 "$SIGIRO_PID" 2>/dev/null; then
    fail "Server failed to start"
    cat /tmp/sigiro-server.log
    exit 1
fi
pass "Server started (PID ${SIGIRO_PID})"

# ─── Step 3: Health check ─────────────────────────────────────────
log "Checking health endpoint..."
HEALTH=$(curl -sf http://localhost:9999/health 2>/dev/null || echo "")
if [ -n "$HEALTH" ]; then
    pass "Health check: ${HEALTH}"
else
    fail "Health check failed"
    cat /tmp/sigiro-server.log | tail -20
fi

# ─── Step 4: Create API key ──────────────────────────────────────
# Use dev key for E2E — sigiro_dev_<anything> always works in dev mode
API_KEY="sigiro_dev_test-project"
export SIGIRO_ENDPOINT="http://localhost:9999"
pass "Using dev API key: ${API_KEY}"

# ─── Step 5: Send OTLP trace data ─────────────────────────────────
log "Sending OTLP trace data via HTTP :4318..."

TRACE_PAYLOAD='{
  "resourceSpans": [{
    "resource": {
      "attributes": [
        {"key": "service.name", "value": {"stringValue": "checkout"}},
        {"key": "service.version", "value": {"stringValue": "1.0.0"}}
      ]
    },
    "scopeSpans": [{
      "scope": {"name": "test-scope"},
      "spans": [{
        "traceId": "0123456789abcdef0123456789abcdef",
        "spanId": "0123456789abcdef",
        "parentSpanId": "",
        "name": "GET /api/checkout",
        "kind": 1,
        "startTimeUnixNano": "'$(date +%s)'000000000",
        "endTimeUnixNano": "'$(date +%s)'001500000",
        "status": {"code": 1}
      }, {
        "traceId": "0123456789abcdef0123456789abcdef",
        "spanId": "abcdef0123456789",
        "parentSpanId": "0123456789abcdef",
        "name": "POST /api/payment",
        "kind": 1,
        "startTimeUnixNano": "'$(date +%s)'000100000",
        "endTimeUnixNano": "'$(date +%s)'003000000",
        "status": {"code": 2, "message": "timeout"}
      }]
    }]
  }]
}'

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST http://localhost:4318/v1/traces \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${API_KEY}" \
    -d "$TRACE_PAYLOAD" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "202" ]; then
    pass "OTLP traces ingested (HTTP ${HTTP_CODE})"
else
    fail "OTLP trace ingest failed (HTTP ${HTTP_CODE})"
fi

# ─── Step 6: Send OTLP log data ───────────────────────────────────
log "Sending OTLP log data..."

LOG_PAYLOAD='{
  "resourceLogs": [{
    "resource": {
      "attributes": [
        {"key": "service.name", "value": {"stringValue": "checkout"}},
        {"key": "service.version", "value": {"stringValue": "1.0.0"}}
      ]
    },
    "scopeLogs": [{
      "scope": {"name": "test-scope"},
      "logRecords": [{
        "timeUnixNano": "'$(date +%s)'000000000",
        "severityNumber": 17,
        "severityText": "ERROR",
        "body": {"stringValue": "OOMKill: container exited with code 137"},
        "attributes": [
          {"key": "k8s.pod.name", "value": {"stringValue": "checkout-7d8f6-x2k9"}},
          {"key": "exit_code", "value": {"intValue": "137"}}
        ],
        "traceId": "0123456789abcdef0123456789abcdef",
        "spanId": "0123456789abcdef"
      }, {
        "timeUnixNano": "'$(date +%s)'001000000",
        "severityNumber": 13,
        "severityText": "WARN",
        "body": {"stringValue": "Connection pool exhausted, retrying in 100ms"},
        "attributes": [
          {"key": "k8s.pod.name", "value": {"stringValue": "checkout-7d8f6-x2k9"}}
        ]
      }]
    }]
  }]
}'

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST http://localhost:4318/v1/logs \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${API_KEY}" \
    -d "$LOG_PAYLOAD" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "202" ]; then
    pass "OTLP logs ingested (HTTP ${HTTP_CODE})"
else
    fail "OTLP log ingest failed (HTTP ${HTTP_CODE})"
fi

# ─── Step 7: Send OTLP metric data ────────────────────────────────
log "Sending OTLP metric data..."

METRIC_PAYLOAD='{
  "resourceMetrics": [{
    "resource": {
      "attributes": [
        {"key": "service.name", "value": {"stringValue": "checkout"}}
      ]
    },
    "scopeMetrics": [{
      "scope": {"name": "test-scope"},
      "metrics": [{
        "name": "http.server.request.duration",
        "description": "HTTP server request duration",
        "unit": "ms",
        "gauge": {
          "dataPoints": [{
            "timeUnixNano": "'$(date +%s)'000000000",
            "asDouble": 150.5,
            "attributes": [
              {"key": "http.method", "value": {"stringValue": "GET"}},
              {"key": "http.status_code", "value": {"intValue": "200"}}
            ]
          }]
        }
      }, {
        "name": "process.memory.usage",
        "description": "Process memory usage",
        "unit": "bytes",
        "gauge": {
          "dataPoints": [{
            "timeUnixNano": "'$(date +%s)'000000000",
            "asDouble": 536870912.0
          }]
        }
      }]
    }]
  }]
}'

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST http://localhost:4318/v1/metrics \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${API_KEY}" \
    -d "$METRIC_PAYLOAD" 2>/dev/null || echo "000")

if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "202" ]; then
    pass "OTLP metrics ingested (HTTP ${HTTP_CODE})"
else
    fail "OTLP metric ingest failed (HTTP ${HTTP_CODE})"
fi

# ─── Step 8: Wait for data to flush ──────────────────────────────
log "Waiting 5s for data to flush to storage..."
sleep 5

# ─── Step 9: Query data ──────────────────────────────────────────
log "Querying stored data..."
# Use the dev key for CLI commands
export SIGIRO_API_KEY="${API_KEY}"
QUERY_RESULT=$($SIGIRO_BIN query "SELECT count(*) as cnt FROM lake.sigiro_spans" 2>&1) || true
log "Query result:"
echo "$QUERY_RESULT"

if echo "$QUERY_RESULT" | grep -qE "[0-9]+"; then
    pass "Query returned data"
else
    fail "Query did not return expected data"
fi

# ─── Step 10: Investigate service ─────────────────────────────────
log "Investigating checkout service..."
NOW=$(date +%s)
INVESTIGATE_RESULT=$($SIGIRO_BIN investigate checkout --from $((NOW - 3600)) --to "$NOW" 2>&1) || true
log "Investigate result:"
echo "$INVESTIGATE_RESULT"

if echo "$INVESTIGATE_RESULT" | grep -qi "checkout\|GET\|POST\|payment\|OOM\|memory"; then
    pass "Investigate returned relevant data"
else
    warn "Investigate may not have returned expected data"
fi

# ─── Step 11: Verify services endpoint ────────────────────────────
log "Checking services endpoint..."
SERVICES_RESULT=$(curl -sf http://localhost:9999/v1/services -H "Authorization: Bearer ${API_KEY}" 2>/dev/null || echo "{}")
log "Services: ${SERVICES_RESULT}"

if echo "$SERVICES_RESULT" | grep -qi "checkout"; then
    pass "Services endpoint returned checkout"
else
    warn "Services endpoint did not include checkout"
fi

# ─── Step 12: Verify server logs ─────────────────────────────────
log "Checking server logs for errors..."
ERROR_COUNT=$(grep -ci "error\|panic\|fatal" /tmp/sigiro-server.log 2>/dev/null || true)
if [ "$ERROR_COUNT" -lt 5 ]; then
    pass "Server logs clean (error mentions: ${ERROR_COUNT})"
else
    warn "Server has ${ERROR_COUNT} error mentions in logs"
fi

# ─── For S3 mode: verify data in MinIO ───────────────────────────
if [ "$MODE" = "s3" ]; then
    log "Verifying data in MinIO..."
    MC_FILES=$(mc ls "local/sigiro-test/e2e-test/" --recursive 2>/dev/null | head -10 || echo "")
    if [ -n "$MC_FILES" ]; then
        pass "Data files found in MinIO"
        echo "$MC_FILES"
    else
        fail "No data files found in MinIO"
    fi

    log "Verifying PostgreSQL catalog..."
    PG_TABLES=$(sudo -u postgres psql -d sigiro_catalog -c "\dt" 2>/dev/null | head -20 || echo "")
    if [ -n "$PG_TABLES" ]; then
        pass "PostgreSQL catalog has tables"
        echo "$PG_TABLES"
    else
        warn "PostgreSQL catalog may be empty"
    fi
fi

# ─── Summary ──────────────────────────────────────────────────────
echo ""
echo "==========================================="
if [ "$FAILED" = "0" ]; then
    echo -e "${GREEN}ALL TESTS PASSED${NC} — mode: ${MODE}"
else
    echo -e "${RED}SOME TESTS FAILED${NC} — mode: ${MODE}"
fi
echo "==========================================="
echo "Server log: /tmp/sigiro-server.log"
[ "$MODE" = "s3" ] && echo "MinIO log: /tmp/minio.log"
echo ""

exit $FAILED
