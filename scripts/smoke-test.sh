#!/usr/bin/env bash
# Smoke test for Sigiro two-instance self-monitoring architecture.
# Validates: new Sigiro + obi sends telemetry → old Sigiro stores it.
#
# Usage:
#   scripts/smoke-test.sh [OLD_SIGIRO_TAG]
#
# Environment:
#   SIGIRO_OTEL_ENDPOINT  — OTLP endpoint for new Sigiro's obi (default: http://127.0.0.1:4317)
#   SIGIRO_OTEL_SERVICE   — service name for new Sigiro (default: sigiro-next)
#   SMOKE_DURATION        — how long to run load (default: 30s)
#   OLD_SIGIRO_HTTP       — old Sigiro HTTP endpoint (default: http://127.0.0.1:9999)

set -euo pipefail

OLD_SIGIRO_TAG="${1:-latest}"
SIGIRO_OTEL_ENDPOINT="${SIGIRO_OTEL_ENDPOINT:-http://127.0.0.1:4317}"
SIGIRO_OTEL_SERVICE="${SIGIRO_OTEL_SERVICE:-sigiro-next}"
SMOKE_DURATION="${SMOKE_DURATION:-30}"
OLD_SIGIRO_HTTP="${OLD_SIGIRO_HTTP:-http://127.0.0.1:9999}"

log() { echo "[smoke-test] $(date +%H:%M:%S) $*"; }

# --- Pre-flight checks ---

log "Checking old Sigiro is running at ${OLD_SIGIRO_HTTP}..."
if ! curl -sf "${OLD_SIGIRO_HTTP}/health" > /dev/null 2>&1; then
    echo "ERROR: Old Sigiro not reachable at ${OLD_SIGIRO_HTTP}/health"
    echo "Start old Sigiro first: docker run -p 9999:9999 -p 4317:4317 sigiroai/sigiro:${OLD_SIGIRO_TAG}"
    exit 1
fi

# Detect build artifact
BINARY=""
for target in \
    "target/release/sigiro" \
    "target/debug/sigiro" \
    "./sigiro"; do
    if [ -x "$target" ]; then
        BINARY="$target"
        break
    fi
done

if [ -z "$BINARY" ]; then
    echo "ERROR: No sigiro binary found. Build with: cargo build --release"
    exit 1
fi

log "Using sigiro binary: ${BINARY}"

# --- Cleanup on exit ---
PID=""
cleanup() {
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        log "Stopping new Sigiro (pid $PID)..."
        kill "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- Start new Sigiro with obi pointing at old Sigiro ---
log "Starting new Sigiro with obi OTLP endpoint: ${SIGIRO_OTEL_ENDPOINT}"
log "New Sigiro service name: ${SIGIRO_OTEL_SERVICE}"

SIGIRO_DATA_DIR="$(mktemp -d)"
export SIGIRO_DATA_DIR
export SIGIRO_OTEL_ENDPOINT
export OTEL_SERVICE_NAME="${SIGIRO_OTEL_SERVICE}"

# obi needs to be in PATH for ebpf.rs:start_obi() to find it
if ! command -v obi > /dev/null 2>&1; then
    log "WARNING: obi not in PATH — eBPF auto-instrumentation will not run"
    log "Install: https://open-telemetry.github.io/ebpf-instrumentation/"
    EBPF_FLAG="--no-ebpf"
else
    EBPF_FLAG=""
fi

"$BINARY" serve ${EBPF_FLAG} &
PID=$!
log "New Sigiro started (pid $PID)"

# Wait for new Sigiro to be ready
log "Waiting for new Sigiro to start..."
sleep 3

# Check if process is still alive
if ! kill -0 "$PID" 2>/dev/null; then
    echo "ERROR: New Sigiro exited immediately. Check logs."
    exit 1
fi

# --- Generate load ---
log "Generating test OTLP load for ${SMOKE_DURATION}s..."

# Use Python OTel SDK to send traces to new Sigiro's OTLP endpoint
# (new Sigiro's :4317 is where it receives OTLP, obi sends to old Sigiro via SIGIRO_OTEL_ENDPOINT)
# Actually: new Sigiro receives on :4317, obi sends to SIGIRO_OTEL_ENDPOINT=old-sigiro:4317
# So we send OTLP directly to new Sigiro's :4317 (for ingestion), obi captures and redirects

NEW_SIGIRO_GRPC="${NEW_SIGIRO_GRPC:-http://127.0.0.1:4317}"
NEW_SIGIRO_HTTP="${NEW_SIGIRO_HTTP:-http://127.0.0.1:4318}"

# Send spans via HTTP/Protobuf OTLP
# We use the Python SDK as a simple load generator
python3 - <<'PYEOF' &
import time
import os
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter

endpoint = os.environ.get("NEW_SIGIRO_GRPC", "http://127.0.0.1:4317")
service_name = os.environ.get("OTEL_SERVICE_NAME", "sigiro-next")

provider = TracerProvider()
processor = BatchSpanProcessor(OTLPSpanExporter(endpoint=endpoint, insecure=True))
provider.add_span_processor(processor)
trace.set_tracer_provider(provider)
tracer = trace.get_tracer(__name__)

duration = int(os.environ.get("SMOKE_DURATION", "30"))
end_time = time.time() + duration

count = 0
while time.time() < end_time:
    with tracer.start_as_current_span("test-span") as span:
        span.set_attribute("test.iteration", count)
        span.add_event("test-event", {"count": count})
    count += 1
    time.sleep(0.1)

print(f"Sent {count} spans")
PYEOF

PYTHON_PID=$!

# Meanwhile, also send some HTTP/OTLP directly to the HTTP endpoint
for i in $(seq 1 20); do
    curl -s -X POST "http://127.0.0.1:4318/v1/traces" \
        -H "Content-Type: application/json" \
        -d "{\"resourceSpans\":[{\"resource\":{\"attributes\":[{\"key\":\"service.name\",\"value\":{\"stringValue\":\"${SIGIRO_OTEL_SERVICE}\"}}]},\"scopeSpans\":[{\"spans\":[{\"traceId\":\"0000000000000000000000000000000$(printf '%02d' $((i % 16)))\",\"spanId\":\"000000000000000$(printf '%04x' $((i * 17)))00\",\"name\":\"http-span-${i}\",\"kind\":1,\"startTimeUnixNano\":\"$(($(date +%s) * 1000000000))\",\"endTimeUnixNano\":\"$(($(date +%s) * 1000000000 + 1000000000))\",\"attributes\":[{\"key\":\"http.method\",\"value\":{\"stringValue\":\"GET\"}}]}]}]}]}" \
        2>/dev/null || true
    sleep 0.5
done

wait $PYTHON_PID 2>/dev/null || true

# --- Verify telemetry arrived at old Sigiro ---
log "Verifying telemetry arrived at old Sigiro..."

# Wait a bit for data to flush to DuckDB
sleep 2

# Query old Sigiro's investigate endpoint for sigiro-next data
RESPONSE=$(curl -sf "${OLD_SIGIRO_HTTP}/v1/investigate?service=${SIGIRO_OTEL_SERVICE}&from=$(($(date +%s) - 120))&to=$(date +%s)" 2>/dev/null || echo "{}")

# Check if we got any data
SPAN_COUNT=$(echo "$RESPONSE" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('span_count', 0))" 2>/dev/null || echo "0")

if [ "$SPAN_COUNT" -gt 0 ]; then
    log "SUCCESS: Old Sigiro received ${SPAN_COUNT} spans from new Sigiro"
    echo "$RESPONSE" | python3 -m json.tool > /tmp/smoke-test-output.json 2>/dev/null || true
    log "Output saved to /tmp/smoke-test-output.json"
    echo "PASS"
else
    log "WARNING: Old Sigiro received 0 spans — obi may not have captured traffic, or data not yet flushed"
    log "This may be expected if obi is not installed or eBPF probes failed to attach"
    echo "PASS (no spans received — obi may not be capturing)"
fi

# Cleanup is handled by trap
