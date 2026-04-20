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
# Use a fixed location to avoid WAL corruption from leftover temp dirs
SIGIRO_DATA_DIR="/tmp/sigiro-smoke-data"
rm -rf "$SIGIRO_DATA_DIR" 2>/dev/null || true
mkdir -p "$SIGIRO_DATA_DIR"
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
"$SIGIRO_BINARY" serve > "${SIGIRO_DATA_DIR}/sigiro.log" 2>&1 &
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

# --- Wait for DuckDB pool to be initialized before sending spans ---
# DuckDB extensions (ducklake, sqlite, system_stats, fsquery, hostfs) are
# downloaded on first connection to the pool. This can take ~20s on a cold start.
# If we start the loader before the pool is ready, spans are dropped because
# pool.get_or_create() fails and the router silently drops IngestMsg::Traces.
# We detect pool readiness by polling sigiro's log output for the pool init message.
log "Waiting for DuckDB pool initialization (checking sigiro log for 'pool: opened DuckDB connection')..."
SIGIRO_LOG="${SIGIRO_DATA_DIR}/sigiro.log"
POOL_READY=0
for i in $(seq 1 30); do
    if [ -f "$SIGIRO_LOG" ] && grep -q 'opened DuckDB connection' "$SIGIRO_LOG" 2>/dev/null; then
        log "DuckDB pool ready after ${i}s"
        POOL_READY=1
        break
    fi
    sleep 1
done
if [ "$POOL_READY" -eq 0 ]; then
    echo "WARNING: DuckDB pool not ready after 30s — proceeding anyway (loader may drop spans)"
fi

# --- Generate OTLP load ---
log "Generating OTLP load for ${SMOKE_DURATION}s → http://127.0.0.1:4318/v1/traces..."

LOADER_START_S=$(python3 -c "import time; print(int(time.time()))")
LOADER="$(mktemp)"
cat > "$LOADER" <<'PYEOF'
import sys, json, time, random, subprocess

duration = int(sys.argv[1])
service_name = sys.argv[2]
url = sys.argv[3]
start_time = time.time()
end_time = start_time + duration
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
LOADER_END_S=$(python3 -c "import time; print(int(time.time()))")

log "Load generation complete — waiting for data to flush..."
sleep 15

# --- Warmup: trigger DuckDB extension downloads before investigate ---
# Avoids cold-start timeout on first investigate call (~20s to download extensions)
# Use Python timestamps for all time math to stay consistent with span send times
LOAD_START_S=$(python3 -c "import time; print(int(time.time()))")
log "Warming up investigate endpoint (trigger extension downloads)..."
curl -sf -m 30 -X POST \
    "http://127.0.0.1:9999/v1/investigate" \
    -H "Authorization: Bearer sigiro_dev_smoke-test" \
    -H "Content-Type: application/json" \
    -d '{"service":"__warmup__","from_ts":0,"to_ts":1}' \
    > /dev/null 2>&1 || true
sleep 5

# --- Query investigate endpoint ---
# Use Python for all timestamp math — avoids bash date +%s vs Python time.time() mismatch (Bug 3)
# Spans were sent between LOADER_START_S and LOADER_END_S; add margin for flush
INVESTIGATE_JSON=$(python3 - <<PYEOF
import time, subprocess, json

loader_start = ${LOADER_START_S}
loader_end   = ${LOADER_END_S}
# Window: generous 120s before and 120s after to cover extension download delays
from_us = (loader_start - 120) * 1_000_000
to_us   = (loader_end + 120) * 1_000_000

service = '${SIGIRO_SERVICE_NAME}'
r = subprocess.run([
    'curl', '-sf', '-s', '-X', 'POST',
    'http://127.0.0.1:9999/v1/investigate',
    '-H', 'Authorization: Bearer sigiro_dev_smoke-test',
    '-H', 'Content-Type: application/json',
    '-d', json.dumps({'service': service, 'from_ts': from_us, 'to_ts': to_us})
], capture_output=True, text=True)

try:
    result = json.loads(r.stdout)
except Exception:
    result = {'error': r.stdout or 'no output', 'stderr': r.stderr}

print(json.dumps(result))
PYEOF
)

FROM_US=$(python3 -c "print((${LOADER_START_S} - 120) * 1_000_000)")
TO_US=$(python3 -c "print((${LOADER_END_S} + 120) * 1_000_000)")

# investigate call is inside the Python block above (Bug 3 fix: use Python timestamps for consistency)
log "Investigate JSON captured via Python (Bug 3 fix: consistent time.time() timestamps)"

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

investigate = json.loads('''${INVESTIGATE_JSON}''')
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
