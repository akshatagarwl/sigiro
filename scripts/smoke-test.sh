#!/usr/bin/env bash
# Smoke test for Sigiro self-monitoring + profiling loop.
#
# Architecture:
#   1. Start sigiro (spawns obi as child to watch OTLP port 4318)
#   2. obi captures traffic and stores traces in sigiro's own DuckDB
#   3. Send OTLP load → sigiro → DuckDB
#   4. Query /v1/investigate → return JSON blocks to AI agent
#   5. Optionally fetch Pyroscope profiles (if SIGIRO_PYROSCOPE_ENDPOINT is set)
#
# Usage:
#   scripts/smoke-test.sh
#
# Environment:
#   SIGIRO_OTEL_ENDPOINT     obi's OTLP destination (default: http://127.0.0.1:9999)
#   SIGIRO_PYROSCOPE_ENDPOINT Pyroscope endpoint for profiling (default: none)
#   SIGIRO_SERVICE_NAME      service name for spans (default: sigiro-self)
#   SMOKE_DURATION            load duration in seconds (default: 30)
#   SIGIRO_BINARY             path to sigiro binary (default: auto-detect)

set -euo pipefail

SIGIRO_OTEL_ENDPOINT="${SIGIRO_OTEL_ENDPOINT:-http://127.0.0.1:9999}"
SIGIRO_PYROSCOPE_ENDPOINT="${SIGIRO_PYROSCOPE_ENDPOINT:-}"
SIGIRO_SERVICE_NAME="${SIGIRO_SERVICE_NAME:-sigiro-self}"
OTEL_AUTH_HEADER="${OTEL_AUTH_HEADER:-Authorization: Bearer sigiro_dev_smoke-test}"
SMOKE_DURATION="${SMOKE_DURATION:-30}"
SIGIRO_BINARY="${SIGIRO_BINARY:-}"

log() { echo "[smoke-test] $(date +%H:%M:%S) $*"; }

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
    echo "ERROR: sigiro binary not found. Build: cargo build [--release]"
    exit 1
fi

log "Using sigiro binary: ${SIGIRO_BINARY}"

# --- Data directory for this run ---
SIGIRO_DATA_DIR="$(mktemp -d)"
export SIGIRO_DATA_DIR
export SIGIRO_OTEL_ENDPOINT
export OTEL_SERVICE_NAME="${SIGIRO_SERVICE_NAME}"

cleanup() {
    log "Stopping sigiro (pid $SIGIRO_PID)..."
    if [ -n "$SIGIRO_PID" ] && kill -0 "$SIGIRO_PID" 2>/dev/null; then
        kill "$SIGIRO_PID" 2>/dev/null || true
        wait "$SIGIRO_PID" 2>/dev/null || true
    fi
    rm -rf "$SIGIRO_DATA_DIR"
}
trap cleanup EXIT

# --- Start sigiro ---
# sigiro serve will:
#   - Start HTTP API on :9999, OTLP HTTP on :4318, OTLP gRPC on :4317
#   - Spawn obi as child (if obi is in PATH and eBPF is available)
#   - obi watches port 4318 and sends captured traces back to :9999 (SIGIRO_OTEL_ENDPOINT)
#   - All traces land in sigiro's own DuckDB via the normal ingest pipeline
log "Starting sigiro (service=${SIGIRO_SERVICE_NAME}, obi→${SIGIRO_OTEL_ENDPOINT})..."
"$SIGIRO_BINARY" serve &
SIGIRO_PID=$!

log "Waiting for sigiro to start (pid $SIGIRO_PID)..."
sleep 4

if ! kill -0 "$SIGIRO_PID" 2>/dev/null; then
    echo "ERROR: sigiro exited immediately after start"
    exit 1
fi

# --- Wait for sigiro to be ready ---
READY_WAIT=0
until curl -sf "http://127.0.0.1:9999/health" > /dev/null 2>&1; do
    sleep 1
    READY_WAIT=$((READY_WAIT + 1))
    if [ "$READY_WAIT" -ge 15 ]; then
        echo "ERROR: sigiro HTTP API not ready after 15s"
        exit 1
    fi
done
log "sigiro HTTP API ready on :9999"

# --- Generate OTLP load ---
log "Generating OTLP load for ${SMOKE_DURATION}s → http://127.0.0.1:4318/v1/traces..."

LOADER="$(mktemp)"
cat > "$LOADER" <<'PYEOF'
import sys, json, time, random, subprocess

duration = int(sys.argv[1])
service_name = sys.argv[2]
url = sys.argv[3]
end_time = time.time() + duration
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
                        {"key": "test.iteration", "value": {"intValue": str(count)}},
                        {"key": "test.load_ms", "value": {"intValue": str(int(random.random() * 50))}}
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
             "-H", sys.argv[4],
             "-d", json.dumps(trace)],
            timeout=5, check=False
        )
    except Exception:
        pass
    count += 1
    time.sleep(0.1)

print(f"Sent {count} spans", file=sys.stderr)
PYEOF

python3 "$LOADER" "$SMOKE_DURATION" "$SIGIRO_SERVICE_NAME" "http://127.0.0.1:4318/v1/traces" "$OTEL_AUTH_HEADER"
rm -f "$LOADER"

log "Load generation complete — waiting for data to flush..."
sleep 5

# --- Warmup: trigger DuckDB extension downloads before investigate ---
# Avoids cold-start timeout on first investigate call (~20s to download extensions)
log "Warming up investigate endpoint (trigger extension downloads)..."
curl -sf -m 30 -X POST \
    "http://127.0.0.1:9999/v1/investigate" \
    -H "Authorization: Bearer sigiro_dev_smoke-test" \
    -H "Content-Type: application/json" \
    -d '{"service":"__warmup__","from_ts":0,"to_ts":1}' \
    > /dev/null 2>&1 || true
sleep 2

# --- Query investigate endpoint ---
NOW="$(date +%s)"
# investigate API uses microseconds and POST with JSON body
FROM_US=$(( (NOW - SMOKE_DURATION - 30) * 1000000 ))
TO_US=$(( (NOW + 10) * 1000000 ))

log "Querying POST /v1/investigate (service=${SIGIRO_SERVICE_NAME}, from_ts=${FROM_US}, to_ts=${TO_US})..."
INVESTIGATE_RESP=$(curl -sf -s \
    -X POST \
    -H "Authorization: Bearer sigiro_dev_smoke-test" \
    -H "Content-Type: application/json" \
    "http://127.0.0.1:9999/v1/investigate" \
    -d "{\"service\":\"${SIGIRO_SERVICE_NAME}\",\"from_ts\":${FROM_US},\"to_ts\":${TO_US}}" \
    2>/dev/null || echo '{"error": "investigate endpoint not available"}')

# --- Optional: fetch Pyroscope profiles ---
PYROSCOPE_PROFILES="null"
if [ -n "$SIGIRO_PYROSCOPE_ENDPOINT" ]; then
    log "Fetching Pyroscope profiles from ${SIGIRO_PYROSCOPE_ENDPOINT}..."
    PYROSCOPE_PROFILES=$(python3 -c "
import urllib.request, json, sys

endpoint = '${SIGIRO_PYROSCOPE_ENDPOINT}'
service = '${SIGIRO_SERVICE_NAME}'
from_us = ${FROM_US}
to_us = ${TO_US}
from_s = from_us // 1000000000
to_s = to_us // 1000000000

def fetch_profile(profile_type, label):
    url = endpoint + '/pyroscope/render?query=%7Bservice_name%3D%22' + service + '%22%7D&profileType=' + profile_type + '&from=' + str(from_s) + '&to=' + str(to_s)
    try:
        with urllib.request.urlopen(url, timeout=10) as r:
            data = json.loads(r.read())
            if data.get('flamegraph') and data['flamegraph'].get('nodes'):
                return {
                    'type': label,
                    'top_functions': [
                        {'name': n['name'], 'value': n.get('value', 0)}
                        for n in data['flamegraph']['nodes'][:10]
                    ]
                }
    except Exception as e:
        return {'type': label, 'error': str(e)}
    return {'type': label, 'error': 'no data'}

profiles = {
    'cpu': fetch_profile('process_cpu:cpu:nanoseconds:cpu:nanoseconds', 'cpu'),
    'memory_inuse': fetch_profile('memory:inuse_space:bytes:space:bytes', 'memory_inuse'),
    'memory_alloc': fetch_profile('memory:alloc_space:bytes:space:bytes', 'memory_alloc'),
}
print(json.dumps(profiles))
" 2>/dev/null || echo '{"error": "pyroscope fetch failed"}')
fi

# --- Build output JSON ---
OUTPUT=$(python3 -c "
import json, sys

investigate = json.loads('''${INVESTIGATE_RESP}''')
pyroscope = json.loads('''${PYROSCOPE_PROFILES}''')

output = {
    'smoke_test': {
        'service': '${SIGIRO_SERVICE_NAME}',
        'duration_seconds': ${SMOKE_DURATION},
        'sigiro_pid': ${SIGIRO_PID},
        'sigiro_data_dir': '${SIGIRO_DATA_DIR}',
        'timestamp': '$(date -u +%Y-%m-%dT%H:%M:%SZ)'
    },
    'investigate': investigate,
    'pyroscope': pyroscope,
}

print(json.dumps(output, indent=2))
")

echo "$OUTPUT" | python3 -m json.tool 2>/dev/null || echo "$OUTPUT"

# --- Store output for cron job delivery ---
echo "$OUTPUT" > /tmp/sigiro-smoke-output.json
log "Output saved to /tmp/sigiro-smoke-output.json"
