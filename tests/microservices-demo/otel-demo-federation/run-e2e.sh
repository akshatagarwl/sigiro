#!/usr/bin/env bash
# Sigiro Fast Federation E2E Test — OpenTelemetry Demo
# Runs in ~90-120s total (vs 3-5min for VM-based test)
#
# Architecture:
#   sigiro-central  ←── quack ATTACH ──→ sigiro-collector ←── OTLP ──→ otel-demo services
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
SIGIRO_BIN="${SIGIRO_BIN:-${REPO_ROOT}/target/release/sigiro}"
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
    cd "$SCRIPT_DIR"
    docker compose down -v --remove-orphans --timeout 10 2>/dev/null || true
}
trap cleanup EXIT

cd "$SCRIPT_DIR"

# ─── Step 0: Prerequisites ───────────────────────────────────────
log "Sigiro Fast Federation E2E Test"

if ! command -v docker &>/dev/null; then
    fail "docker not found"
    exit 1
fi

# Check for sigiro binary — build image if needed
if [ ! -f "$SIGIRO_BIN" ]; then
    log "Building sigiro binary..."
    cd "$REPO_ROOT"
    cargo build --release --bin sigiro
    cd "$SCRIPT_DIR"
fi

# Build minimal sigiro Docker image if not present
if ! docker image inspect sigiro:local &>/dev/null 2>&1; then
    log "Building sigiro:local Docker image..."
    cp "$SIGIRO_BIN" ./sigiro
    docker build -f Dockerfile.sigiro -t sigiro:local .
    rm -f ./sigiro
fi
pass "sigiro:local image ready"

# Check for product-catalog custom image
if ! docker image inspect product-catalog:sigiro &>/dev/null 2>&1; then
    warn "product-catalog:sigiro image not found. Building from submodule..."
    cd "$REPO_ROOT/tests/microservices-demo/opentelemetry-demo/src/product-catalog"
    docker build -t product-catalog:sigiro -f Dockerfile .
    cd "$SCRIPT_DIR"
fi
pass "product-catalog:sigiro image ready"

# ─── Step 1: Start the stack ─────────────────────────────────────
log "Starting sigiro federation + OpenTelemetry Demo..."
docker compose up -d --wait --wait-timeout 120

# Give a moment for registration
sleep 3

# ─── Step 2: Verify federation workers ───────────────────────────
log "Verifying federation workers..."
for i in {1..30}; do
    WORKERS=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
        "http://localhost:9999/v1/federation/workers" 2>/dev/null | \
        python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('workers',[])))" 2>/dev/null || echo "0")
    if [ "$WORKERS" -ge 1 ]; then
        pass "Federation: $WORKERS worker(s) registered"
        break
    fi
    sleep 1
done

if [ "${WORKERS:-0}" -lt 1 ]; then
    fail "No federation workers registered"
    curl -s -H "Authorization: Bearer sigiro_dev_e2e" \
        "http://localhost:9999/v1/federation/workers" 2>/dev/null || true
fi

# ─── Step 3: Wait for spans with polling ─────────────────────────
log "Waiting for telemetry to flow..."
SPANS=0
LOGS=0
METRICS=0
for i in {1..60}; do
    SPANS=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
        -H "Content-Type: text/plain" \
        -d "SELECT COUNT(*) FROM sigiro_spans" \
        "http://localhost:9999/v1/query" 2>/dev/null | \
        python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0].get('count_star()','0') if isinstance(d,list) and d else '0')" 2>/dev/null || echo "0")

    LOGS=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
        -H "Content-Type: text/plain" \
        -d "SELECT COUNT(*) FROM sigiro_logs" \
        "http://localhost:9999/v1/query" 2>/dev/null | \
        python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0].get('count_star()','0') if isinstance(d,list) and d else '0')" 2>/dev/null || echo "0")

    METRICS=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
        -H "Content-Type: text/plain" \
        -d "SELECT COUNT(*) FROM sigiro_metrics_sum" \
        "http://localhost:9999/v1/query" 2>/dev/null | \
        python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0].get('count_star()','0') if isinstance(d,list) and d else '0')" 2>/dev/null || echo "0")

    if [ "$SPANS" -ge 100 ] && [ "$LOGS" -ge 10 ] && [ "$METRICS" -ge 10 ]; then
        pass "Telemetry flowing: spans=$SPANS logs=$LOGS metrics=$METRICS"
        break
    fi
    echo -n "."
    sleep 1
done
echo ""

if [ "$SPANS" -lt 100 ]; then
    fail "Not enough spans: $SPANS (expected >= 100)"
fi
if [ "$LOGS" -lt 10 ]; then
    warn "Not enough logs: $LOGS (expected >= 10)"
fi
if [ "$METRICS" -lt 10 ]; then
    warn "Not enough metrics: $METRICS (expected >= 10)"
fi

# ─── Step 4: Verify services ─────────────────────────────────────
log "Checking detected services..."
SERVICES=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
    -H "Content-Type: text/plain" \
    -d "SELECT DISTINCT service_name FROM sigiro_spans ORDER BY service_name" \
    "http://localhost:9999/v1/query" 2>/dev/null | \
    python3 -c "import sys,json; d=json.load(sys.stdin); print(','.join([r['service_name'] for r in d]) if isinstance(d,list) else '')" 2>/dev/null || echo "")

log "Services: $SERVICES"

EXPECTED="frontend,frontend-proxy,cart,checkout,payment,product-catalog"
MISSING=""
for SVC in frontend frontend-proxy cart checkout payment product-catalog; do
    if ! echo "$SERVICES" | grep -qw "$SVC"; then
        MISSING="$MISSING $SVC"
    fi
done

if [ -z "$MISSING" ]; then
    pass "All expected services detected"
else
    fail "Missing services:$MISSING"
fi

# ─── Step 5: Federated query ─────────────────────────────────────
log "Testing federated query..."
FED_RESULT=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
    -H "Content-Type: text/plain" \
    -d "SELECT service_name, COUNT(*) as c FROM sigiro_spans GROUP BY service_name ORDER BY c DESC LIMIT 5" \
    "http://localhost:9999/v1/query" 2>/dev/null || echo "[]")

if echo "$FED_RESULT" | grep -q "frontend-proxy\|cart"; then
    pass "Federated query returned data"
else
    fail "Federated query failed or empty"
fi

# ─── Step 6: Distributed trace ───────────────────────────────────
log "Checking distributed traces..."
TRACE=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
    -H "Content-Type: text/plain" \
    -d "SELECT trace_id FROM sigiro_spans WHERE service_name = 'frontend-proxy' LIMIT 1" \
    "http://localhost:9999/v1/query" 2>/dev/null | \
    python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0]['trace_id'] if isinstance(d,list) and d else '')" 2>/dev/null || echo "")

if [ -n "$TRACE" ]; then
    TRACE_DETAIL=$(curl -sf -H "Authorization: Bearer sigiro_dev_e2e" \
        "http://localhost:9999/v1/traces/$TRACE" 2>/dev/null || echo "")
    if echo "$TRACE_DETAIL" | grep -q "trace_id"; then
        pass "Trace retrieval works (trace_id=$TRACE)"
    else
        warn "Trace retrieval failed for $TRACE"
    fi
else
    warn "No frontend-proxy traces found for trace retrieval test"
fi

# ─── Step 7: Container health ────────────────────────────────────
log "Checking container health..."
UNHEALTHY=$(docker compose ps --format json 2>/dev/null | python3 -c "
import sys, json
containers = json.load(sys.stdin) if sys.stdin.read().strip().startswith('[') else []
if not isinstance(containers, list): containers = [containers]
for c in containers:
    if c.get('State') != 'running':
        print(f\"{c.get('Name','?')}: {c.get('State','?')}\")
" 2>/dev/null || echo "")

if [ -z "$UNHEALTHY" ]; then
    pass "All containers running"
else
    warn "Some containers not running:"
    echo "$UNHEALTHY"
fi

# ─── Summary ─────────────────────────────────────────────────────
echo ""
echo "==========================================="
if [ "$FAILED" = "0" ]; then
    echo -e "${GREEN}Fast Federation E2E: ALL CHECKS PASSED${NC}"
else
    echo -e "${RED}Fast Federation E2E: SOME CHECKS FAILED${NC}"
fi
echo "==========================================="
echo "Spans:  $SPANS"
echo "Logs:   $LOGS"
echo "Metrics: $METRICS"
echo "Services: $SERVICES"
echo ""

exit $FAILED
