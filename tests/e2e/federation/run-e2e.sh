#!/usr/bin/env bash
# Sigiro Fast Federation E2E Test — OpenTelemetry Demo
# Runs in ~90-120s total (vs 3-5min for VM-based test)
#
# Architecture:
#   sigiro-central  ←── quack ATTACH ──→ sigiro-collector ←── OTLP ──→ otel-demo services
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
# Use cross-compiled Linux binary by default on macOS, fallback to native release binary
if [ "$(uname -s)" = "Darwin" ] && [ -f "${REPO_ROOT}/target/aarch64-unknown-linux-gnu/release/sigiro" ]; then
    SIGIRO_BIN="${SIGIRO_BIN:-${REPO_ROOT}/target/aarch64-unknown-linux-gnu/release/sigiro}"
else
    SIGIRO_BIN="${SIGIRO_BIN:-${REPO_ROOT}/target/release/sigiro}"
fi
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

if ! command -v jq &>/dev/null; then
    fail "jq not found (required for JSON parsing). Install: https://jqlang.github.io/jq/download/"
    exit 1
fi

if ! docker compose version &>/dev/null; then
    fail "docker compose plugin not found"
    exit 1
fi

# Auto-init git submodules if the opentelemetry-demo directory is empty
if [ ! -f "$REPO_ROOT/tests/fixtures/opentelemetry-demo/.git" ]; then
    log "Initializing git submodules..."
    cd "$REPO_ROOT"
    git submodule update --init --recursive
    cd "$SCRIPT_DIR"
fi

# Generate .env if missing — base it on upstream demo .env and append sigiro overrides
if [ ! -f ".env" ]; then
    log "Generating .env from upstream template..."
    UPSTREAM_ENV="$REPO_ROOT/tests/fixtures/opentelemetry-demo/.env"
    if [ -f "$UPSTREAM_ENV" ]; then
        cp "$UPSTREAM_ENV" .env
    fi
    cat >> .env <<'EOF'

# Sigiro Federation overrides
SIGIRO_API_KEY=sigiro_dev_e2e
SIGIRO_CENTRAL_URL=http://sigiro-central:9999
EOF
    pass ".env generated"
fi

# Load SIGIRO_API_KEY from .env for use throughout the script
SIGIRO_API_KEY="${SIGIRO_API_KEY:-$(grep '^SIGIRO_API_KEY=' .env | cut -d= -f2-)}"
export SIGIRO_API_KEY

# Check for sigiro binary — build from source if missing and not skipped
if [ "${SIGIRO_SKIP_BUILD:-}" = "1" ]; then
    if [ ! -f "$SIGIRO_BIN" ]; then
        fail "SIGIRO_SKIP_BUILD=1 but binary not found at $SIGIRO_BIN"
        exit 1
    fi
else
    if [ ! -f "$SIGIRO_BIN" ]; then
        if ! command -v cargo &>/dev/null; then
            fail "cargo not found. Install Rust or set SIGIRO_BIN to a pre-built binary."
            exit 1
        fi
        log "Building sigiro binary (this may take a few minutes)..."
        cd "$REPO_ROOT"
        cargo build --release --bin sigiro
        cd "$SCRIPT_DIR"
    fi
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
    cd "$REPO_ROOT/tests/fixtures/opentelemetry-demo/src/product-catalog"
    docker build -t product-catalog:sigiro -f Dockerfile .
    cd "$SCRIPT_DIR"
fi
pass "product-catalog:sigiro image ready"

# ─── Step 1: Start the stack ─────────────────────────────────────
log "Starting sigiro federation + OpenTelemetry Demo..."
docker compose up -d --wait --wait-timeout 300

# Wait for key demo services to be running (not just created)
log "Waiting for demo services to start..."
for i in $(seq 1 60); do
    RUNNING=$(docker compose ps --format '{{.Name}}' --filter status=running 2>/dev/null | wc -l | tr -d ' ')
    if [ "$RUNNING" -ge 15 ]; then
        pass "$RUNNING containers running"
        break
    fi
    echo -n "."
    sleep 1
done
echo ""

# ─── Step 2: Verify federation workers ───────────────────────────
log "Verifying federation workers..."
for i in {1..30}; do
    WORKERS=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
        "http://localhost:9999/v1/federation/workers" 2>/dev/null | \
        jq '.workers | length' 2>/dev/null || echo "0")
    if [ "$WORKERS" -ge 1 ]; then
        pass "Federation: $WORKERS worker(s) registered"
        break
    fi
    sleep 1
done

if [ "${WORKERS:-0}" -lt 1 ]; then
    fail "No federation workers registered"
    curl -s -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
        "http://localhost:9999/v1/federation/workers" 2>/dev/null || true
fi

# ─── Step 3: Wait for spans with polling ─────────────────────────
log "Waiting for telemetry to flow..."
SPANS=0
LOGS=0
METRICS=0
# Allow extra time for DuckDB extension download on first boot + OTLP flow + federation sync
for i in {1..180}; do
    SPANS=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
        -H "Content-Type: text/plain" \
        -d "SELECT COUNT(*) FROM sigiro_spans" \
        "http://localhost:9999/v1/query" 2>/dev/null | \
        jq -r '.[0]."count_star()" // 0' 2>/dev/null || echo "0")

    LOGS=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
        -H "Content-Type: text/plain" \
        -d "SELECT COUNT(*) FROM sigiro_logs" \
        "http://localhost:9999/v1/query" 2>/dev/null | \
        jq -r '.[0]."count_star()" // 0' 2>/dev/null || echo "0")

    METRICS=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
        -H "Content-Type: text/plain" \
        -d "SELECT COUNT(*) FROM sigiro_metrics_sum" \
        "http://localhost:9999/v1/query" 2>/dev/null | \
        jq -r '.[0]."count_star()" // 0' 2>/dev/null || echo "0")

    if [ "$SPANS" -ge 50 ] && [ "$LOGS" -ge 5 ] && [ "$METRICS" -ge 5 ]; then
        pass "Telemetry flowing: spans=$SPANS logs=$LOGS metrics=$METRICS"
        break
    fi
    if [ $((i % 10)) -eq 0 ]; then
        echo -n "${i}s"
    else
        echo -n "."
    fi
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
SERVICES=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
    -H "Content-Type: text/plain" \
    -d "SELECT DISTINCT service_name FROM sigiro_spans ORDER BY service_name" \
    "http://localhost:9999/v1/query" 2>/dev/null | \
    jq -r '[.[].service_name] | join(",")' 2>/dev/null || echo "")

log "Services: $SERVICES"

# Core services we reliably detect (upstream demo checkout has a known nil-ptr panic
# in v2.2.0, and product-catalog/payment may not appear until checkout is healthy).
EXPECTED_CORE="frontend-proxy,cart"
MISSING=""
for SVC in frontend-proxy cart; do
    if ! echo "$SERVICES" | grep -qw "$SVC"; then
        MISSING="$MISSING $SVC"
    fi
done

# Bonus: log any additional expected services that are present
BONUS="frontend checkout payment product-catalog"
BONUS_FOUND=""
for SVC in $BONUS; do
    if echo "$SERVICES" | grep -qw "$SVC"; then
        BONUS_FOUND="$BONUS_FOUND $SVC"
    fi
done

if [ -z "$MISSING" ]; then
    pass "Core services detected ($EXPECTED_CORE); bonus:$BONUS_FOUND"
else
    fail "Missing core services:$MISSING"
fi

# ─── Step 5: Federated query ─────────────────────────────────────
log "Testing federated query..."
FED_RESULT=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
    -H "Content-Type: text/plain" \
    -d "SELECT service_name, COUNT(*) FROM sigiro_spans GROUP BY service_name ORDER BY COUNT(*) DESC LIMIT 5" \
    "http://localhost:9999/v1/query" 2>/dev/null || echo "[]")

if echo "$FED_RESULT" | grep -q "frontend-proxy\|cart"; then
    pass "Federated query returned data"
else
    fail "Federated query failed or empty"
fi

# ─── Step 6: Distributed trace ───────────────────────────────────
log "Checking distributed traces..."
TRACE=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
    -H "Content-Type: text/plain" \
    -d "SELECT trace_id FROM sigiro_spans WHERE service_name = 'frontend-proxy' LIMIT 1" \
    "http://localhost:9999/v1/query" 2>/dev/null | \
    jq -r '.[0].trace_id // empty' 2>/dev/null || echo "")

if [ -n "$TRACE" ]; then
    TRACE_DETAIL=$(curl -sf -H "Authorization: Bearer ${SIGIRO_API_KEY}" \
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
TOTAL=$(docker compose ps --format '{{.Name}}' 2>/dev/null | wc -l | tr -d ' ')
RUNNING=$(docker compose ps --format '{{.Name}}' --filter status=running 2>/dev/null | wc -l | tr -d ' ')
if [ "$RUNNING" -eq "$TOTAL" ] && [ "$TOTAL" -gt 0 ]; then
    pass "All $TOTAL containers running"
else
    warn "$RUNNING/$TOTAL containers running"
    docker compose ps --format 'table {{.Name}}	{{.Status}}' 2>/dev/null || true
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
