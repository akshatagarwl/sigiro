#!/usr/bin/env bash
# Sigiro Lima E2E test — run inside Lima VM
# Usage: limactl shell <vm> bash -c 'MODE=local /path/to/run-e2e.sh'
set -euo pipefail

MODE="${MODE:-local}"
BIN="${SIGIRO_BIN:-$HOME/sigiro/bin/sigiro}"
DATA_DIR="/tmp/sigiro-e2e-$$"
FAILED=0
PID_FILE="/tmp/sigiro-e2e-$$-pid"

G='\033[0;32m'; R='\033[0;31m'; Y='\033[0;33m'; C='\033[0;36m'; N='\033[0m'
log()  { echo -e "${C}[$(date +%H:%M:%S)]${N} $*"; }
pass() { echo -e "${G}[PASS]${N} $*"; FAILED=0; }
fail() { echo -e "${R}[FAIL]${N} $*"; FAILED=1; }
warn() { echo -e "${Y}[WARN]${N} $*"; }

cleanup() {
    [ -f "$PID_FILE" ] && kill "$(cat "$PID_FILE")" 2>/dev/null || true
    rm -f "$PID_FILE"
    pkill -x rustfs 2>/dev/null || true
    log "Cleaned up"
}
trap cleanup EXIT

# ── Setup ─────────────────────────────────────────────
log "Sigiro E2E — mode=${MODE}"
[ ! -f "$BIN" ] && { fail "Binary not found at ${BIN}"; exit 1; }

rm -rf "$DATA_DIR" && mkdir -p "$DATA_DIR"
log "Data dir: ${DATA_DIR}"

# ── S3 mode: start RustFS + PostgreSQL ─────────────────
if [ "$MODE" = "s3" ]; then
    log "Starting RustFS..."
    export RUSTFS_ACCESS_KEY=sigiro RUSTFS_SECRET_KEY=sigiro-secret
    mkdir -p /tmp/rustfs-data-$$
    rustfs /tmp/rustfs-data-$$ --address ":9000" --console-address ":9001" > /tmp/rustfs-$$-log 2>&1 &
    sleep 2
    if ! kill -0 $! 2>/dev/null; then fail "RustFS failed"; cat /tmp/rustfs-$$-log; exit 1; fi
    pass "RustFS started"
    rc alias set local http://localhost:9000 sigiro sigiro-secret 2>&1
    rc bucket create --ignore-existing local/sigiro-e2e
    pass "Bucket created"

    log "Starting PostgreSQL..."
    sudo pg_ctlcluster 16 main start 2>/dev/null || true
    sudo -u postgres psql -c "CREATE USER sigiro WITH PASSWORD 'sigiro';" 2>/dev/null || true
    sudo -u postgres psql -c "DROP DATABASE IF EXISTS sigiro_e2e;" 2>/dev/null
    sudo -u postgres psql -c "CREATE DATABASE sigiro_e2e OWNER sigiro;" 2>/dev/null
    pass "PostgreSQL ready"
fi

# ── Start sigiro server ───────────────────────────────
export SIGIRO_DATA_DIR="$DATA_DIR"
export RUST_LOG=sigiro=info

if [ "$MODE" = "s3" ]; then
    export SIGIRO_CATALOG_URL="postgresql://sigiro:sigiro@localhost:5432/sigiro_e2e"
    export SIGIRO_S3_BUCKET="sigiro-e2e"
    export SIGIRO_S3_REGION="us-east-1"
    export AWS_ACCESS_KEY_ID=sigiro
    export AWS_SECRET_ACCESS_KEY=sigiro-secret
    export SIGIRO_S3_ENDPOINT="http://localhost:9000"
fi

$BIN serve > /tmp/sigiro-$$-server.log 2>&1 &
echo $! > "$PID_FILE"
SERVER_PID=$(cat "$PID_FILE")
log "Server PID: ${SERVER_PID}"
sleep 3

if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    fail "Server crashed on startup"
    cat /tmp/sigiro-$$-server.log
    exit 1
fi
pass "Server started"

# ── Health check ───────────────────────────────────────
log "Health check..."
HEALTH=$(curl -sf http://localhost:9999/health 2>/dev/null || echo "")
if [ -n "$HEALTH" ]; then
    pass "Health: ${HEALTH}"
else
    fail "Health check failed"
    tail -20 /tmp/sigiro-$$-server.log
fi

# ── Send OTLP traces ──────────────────────────────────
log "Ingesting traces..."
NOW=$(date +%s)
TRACE="{\"resourceSpans\":[{\"resource\":{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"checkout\"}}]},\"scopeSpans\":[{\"scope\":{\"name\":\"e2e\"},\"spans\":[{\"traceId\":\"aabbccddeeff00112233445566778899\",\"spanId\":\"0011223344556677\",\"name\":\"GET /api/cart\",\"kind\":1,\"startTimeUnixNano\":\"${NOW}000000000\",\"endTimeUnixNano\":\"${NOW}001000000\",\"status\":{\"code\":1}},{\"traceId\":\"aabbccddeeff00112233445566778899\",\"spanId\":\"aabbccddeeff0011\",\"parentSpanId\":\"0011223344556677\",\"name\":\"GET /api/price\",\"kind\":1,\"startTimeUnixNano\":\"${NOW}001000000\",\"endTimeUnixNano\":\"${NOW}005000000\",\"status\":{\"code\":2,\"message\":\"timeout\"}}]}]}]}"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:4318/v1/traces \
    -H "Content-Type: application/json" -H "Authorization: Bearer ${API_KEY}" -d "$TRACE" 2>/dev/null || echo "000")
[ "$CODE" = "200" ] || [ "$CODE" = "202" ] && pass "Traces ingested (HTTP ${CODE})" || fail "Trace ingest failed (HTTP ${CODE})"

# ── Send OTLP logs ────────────────────────────────────
log "Ingesting logs..."
LOGS="{\"resourceLogs\":[{\"resource\":{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"checkout\"}}]},\"scopeLogs\":[{\"scope\":{\"name\":\"e2e\"},\"logRecords\":[{\"timeUnixNano\":\"${NOW}000000000\",\"severityNumber\":17,\"severityText\":\"ERROR\",\"body\":{\"stringValue\":\"OOMKill: exit code 137\"},\"attributes\":[{\"key\":\"exit_code\",\"value\":{\"intValue\":\"137\"}}],\"traceId\":\"aabbccddeeff00112233445566778899\",\"spanId\":\"0011223344556677\"},{\"timeUnixNano\":\"${NOW}005000000\",\"severityNumber\":13,\"severityText\":\"WARN\",\"body\":{\"stringValue\":\"Connection pool exhausted\"}}]}]}]}"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:4318/v1/logs \
    -H "Content-Type: application/json" -H "Authorization: Bearer ${API_KEY}" -d "$LOGS" 2>/dev/null || echo "000")
[ "$CODE" = "200" ] || [ "$CODE" = "202" ] && pass "Logs ingested (HTTP ${CODE})" || fail "Log ingest failed (HTTP ${CODE})"

# ── Send OTLP metrics ─────────────────────────────────
log "Ingesting metrics..."
METRICS="{\"resourceMetrics\":[{\"resource\":{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"checkout\"}}]},\"scopeMetrics\":[{\"scope\":{\"name\":\"e2e\"},\"metrics\":[{\"name\":\"http.server.duration\",\"unit\":\"ms\",\"gauge\":{\"dataPoints\":[{\"timeUnixNano\":\"${NOW}000000000\",\"asDouble\":250.5,\"attributes\":[{\"key\":\"http.method\",\"value\":{\"stringValue\":\"GET\"}}]}]}},{\"name\":\"process.memory\",\"unit\":\"By\",\"gauge\":{\"dataPoints\":[{\"timeUnixNano\":\"${NOW}000000000\",\"asDouble\":536870912.0}]}}]}]}]}"

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST http://localhost:4318/v1/metrics \
    -H "Content-Type: application/json" -H "Authorization: Bearer ${API_KEY}" -d "$METRICS" 2>/dev/null || echo "000")
[ "$CODE" = "200" ] || [ "$CODE" = "202" ] && pass "Metrics ingested (HTTP ${CODE})" || fail "Metric ingest failed (HTTP ${CODE})"

# ── Flush wait ─────────────────────────────────────────
log "Waiting 5s for flush..."
sleep 5

# ── Query verification ────────────────────────────────
log "Querying data..."
QUERY_OUT=$($BIN query "SELECT service, signal, count(*) as cnt FROM lake.sigiro_records GROUP BY service, signal" 2>&1) || true
log "Query result:"
echo "$QUERY_OUT"
echo "$QUERY_OUT" | grep -q "checkout" && pass "Query returned checkout data" || fail "Query missing checkout data"

# ── Diagnose ────────────────────────────────────────
log "Diagnosing checkout..."
INV_OUT=$($BIN diagnose checkout --from $((NOW - 3600)) --to "$NOW" 2>&1) || true
log "Diagnose:"
echo "$INV_OUT"
(echo "$INV_OUT" | grep -qi "checkout\|GET\|POST\|OOM\|memory\|duration\|error") && pass "Diagnose returned data" || warn "Diagnose output may be empty"

# ── Services endpoint ─────────────────────────────────
SERVICES=$(curl -sf http://localhost:9999/services -H "Authorization: Bearer ${API_KEY}" 2>/dev/null || echo "{}")
log "Services: ${SERVICES}"

# ── S3 verification ───────────────────────────────────
if [ "$MODE" = "s3" ]; then
    log "Checking RustFS data files..."
    FILES=$(rc ls local/sigiro-e2e/e2e/ --recursive 2>/dev/null | head -10 || echo "")
    [ -n "$FILES" ] && { pass "RustFS has data files"; echo "$FILES"; } || fail "No RustFS data files"

    log "Checking PostgreSQL catalog..."
    TABLES=$(sudo -u postgres psql -d sigiro_e2e -c "\dt" 2>/dev/null | head -20 || echo "")
    [ -n "$TABLES" ] && { pass "PostgreSQL catalog has tables"; echo "$TABLES"; } || warn "PostgreSQL catalog empty"
fi

# ── Summary ────────────────────────────────────────────
echo ""
echo "========================================="
echo "Server log tail:"
tail -15 /tmp/sigiro-$$-server.log 2>/dev/null
echo "========================================="

if [ "$FAILED" = "0" ]; then
    echo -e "${G}ALL PASSED${N} — mode: ${MODE}"
else
    echo -e "${R}FAILURES DETECTED${N} — mode: ${MODE}"
fi
echo "========================================="
exit $FAILED
