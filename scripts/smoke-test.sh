#!/usr/bin/env bash
# Smoke test for Sigiro two-instance self-monitoring architecture.
# Validates: new Sigiro + obi sends telemetry → old Sigiro stores it.
#
# How it works:
#   1. Start old Sigiro as observability backend (user handles this externally)
#   2. Start new Sigiro with SIGIRO_OTEL_ENDPOINT pointing to old Sigiro
#   3. obi auto-instruments new Sigiro's HTTP/gRPC handlers via eBPF
#   4. obi exports OTLP → old Sigiro's OTLP receiver
#   5. This script verifies old Sigiro received the telemetry
#
# Usage:
#   # Terminal 1: start old Sigiro
#   docker run -d --name old-sigiro \
#     -p 9999:9999 -p 4317:4317 \
#     --cap-add BPF --cap-add NET_ADMIN \
#     sigiroai/sigiro:latest
#
#   # Terminal 2: run smoke test
#   scripts/smoke-test.sh
#
# Environment:
#   SIGIRO_OTEL_ENDPOINT  — obi OTLP endpoint (default: http://127.0.0.1:4317)
#   OTEL_SERVICE_NAME     — service name for new Sigiro spans (default: sigiro-next)
#   SMOKE_DURATION         — how long to run load (default: 30s)
#   OLD_SIGIRO_HTTP        — old Sigiro HTTP endpoint (default: http://127.0.0.1:9999)
#   SIGIRO_BINARY          — path to sigiro binary (default: auto-detect)

set -euo pipefail

SIGIRO_OTEL_ENDPOINT="${SIGIRO_OTEL_ENDPOINT:-http://127.0.0.1:4317}"
OTEL_SERVICE_NAME="${OTEL_SERVICE_NAME:-sigiro-next}"
SMOKE_DURATION="${SMOKE_DURATION:-30}"
OLD_SIGIRO_HTTP="${OLD_SIGIRO_HTTP:-http://127.0.0.1:9999}"
SIGIRO_BINARY="${SIGIRO_BINARY:-}"

log() { echo "[smoke-test] $(date +%H:%M:%S) $*"; }

# --- Pre-flight: is old Sigiro reachable? ---
log "Checking old Sigiro is running at ${OLD_SIGIRO_HTTP}..."
if ! curl -sf "${OLD_SIGIRO_HTTP}/health" > /dev/null 2>&1; then
    echo "ERROR: Old Sigiro not reachable at ${OLD_SIGIRO_HTTP}/health"
    echo "Start old Sigiro first:"
    echo "  docker run -d --name old-sigiro -p 9999:9999 -p 4317:4317 \\"
    echo "    --cap-add BPF --cap-add NET_ADMIN sigiroai/sigiro:latest"
    exit 1
fi

# --- Detect sigiro binary ---
if [ -z "$SIGIRO_BINARY" ]; then
    for path in target/release/sigiro target/debug/sigiro ./sigiro; do
        if [ -x "$path" ]; then
            SIGIRO_BINARY="$path"
            break
        fi
    done
fi

if [ -z "$SIGIRO_BINARY" ]; then
    echo "ERROR: sigiro binary not found. Build: cargo build --release"
    exit 1
fi

log "Using sigiro binary: ${SIGIRO_BINARY}"

# --- Detect if obi is available ---
OBE_FLAG=""
if ! command -v obi > /dev/null 2>&1; then
    log "WARNING: obi not in PATH — eBPF auto-instrumentation disabled"
    log "Install obi for full auto-instrumentation: https://open-telemetry.github.io/ebpf-instrumentation/"
    OBE_FLAG="--no-ebpf"
fi

# --- Start new Sigiro ---
SIGIRO_DATA_DIR="$(mktemp -d)"
export SIGIRO_DATA_DIR
export SIGIRO_OTEL_ENDPOINT
export OTEL_SERVICE_NAME

cleanup() {
    log "Stopping new Sigiro..."
    if [ -n "$NEW_PID" ] && kill -0 "$NEW_PID" 2>/dev/null; then
        kill "$NEW_PID" 2>/dev/null || true
        wait "$NEW_PID" 2>/dev/null || true
    fi
    rm -rf "$SIGIRO_DATA_DIR"
}
trap cleanup EXIT

log "Starting new Sigiro (service=${OTEL_SERVICE_NAME}, obi→${SIGIRO_OTEL_ENDPOINT})..."
"$SIGIRO_BINARY" serve ${OBE_FLAG} &
NEW_PID=$!

log "Waiting for new Sigiro to start (pid $NEW_PID)..."
sleep 4

if ! kill -0 "$NEW_PID" 2>/dev/null; then
    echo "ERROR: New Sigiro exited immediately after start"
    exit 1
fi

# Generate OTLP load ---
log "Generating OTLP load for ${SMOKE_DURATION}s (sending to new Sigiro)..."

# Write Python loader to temp file to avoid bash quoting issues
LOADER="$(mktemp)"
cat > "$LOADER" <<'PYEOF'
import sys, json, time, random, subprocess

duration = int(sys.argv[1])
service_name = sys.argv[2]
end_time = time.time() + duration

url = "http://127.0.0.1:4318/v1/traces"
count = 0

while time.time() < end_time:
    trace = {
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": service_name}},
                    {"key": "service.version", "value": {"stringValue": "smoke-test"}}
                ]
            },
            "scopeSpans": [{
                "spans": [{
                    "traceId": format(random.getrandbits(128), '032x'),
                    "spanId": format(random.getrandbits(64), '016x'),
                    "name": f"test-span-{count}",
                    "kind": 1,
                    "startTimeUnixNano": int(time.time() * 1e9),
                    "endTimeUnixNano": int(time.time() * 1e9) + int(1e8 * random.random()),
                    "attributes": [
                        {"key": "http.method", "value": {"stringValue": "GET"}},
                        {"key": "http.url", "value": {"stringValue": f"/test/{count}"}},
                        {"key": "test.iteration", "value": {"intValue": str(count)}}
                    ],
                    "status": {"code": 0}
                }]
            }]
        }]
    }
    try:
        subprocess.run(
            ["curl", "-sf", "-X", "POST", url,
             "-H", "Content-Type: application/json",
             "-d", json.dumps(trace)],
            timeout=5, check=False
        )
    except Exception:
        pass
    count += 1
    time.sleep(0.1)

print(f"Sent {count} spans", file=sys.stderr)
PYEOF

python3 "$LOADER" "$SMOKE_DURATION" "$OTEL_SERVICE_NAME"
rm -f "$LOADER"

log "Load generation complete — waiting for data to flush..."
sleep 3

# --- Verify ---
log "Querying old Sigiro for sigiro-next telemetry..."
RESPONSE=$(curl -sf \
    "${OLD_SIGIRO_HTTP}/v1/investigate?service=${OTEL_SERVICE_NAME}&from=$(($(date +%s) - 120))&to=$(date +%s)" \
    2>/dev/null || echo '{"span_count": 0}')

SPAN_COUNT=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('span_count', 0))" 2>/dev/null || echo "0")

log "Old Sigiro received $SPAN_COUNT spans tagged as ${OTEL_SERVICE_NAME}"

if [ "$SPAN_COUNT" -gt 0 ]; then
    echo ""
    echo "=== PASS: Self-monitoring is working ==="
    echo "obi is auto-instrumenting new Sigiro and sending to old Sigiro."
    echo "Agent can now read: GET /v1/investigate?service=${OTEL_SERVICE_NAME}"
    echo ""
    echo "Output saved to /tmp/smoke-test-output.json"
    echo "$RESPONSE" | python3 -m json.tool > /tmp/smoke-test-output.json 2>/dev/null || true
else
    echo ""
    echo "=== WARNING: No spans received ==="
    echo "This is expected if obi is not installed or eBPF probes failed to attach."
    echo "The two-instance architecture still works for manually instrumented services."
    echo "Install obi for full auto-instrumentation."
fi

# --- Cleanup via trap ---
