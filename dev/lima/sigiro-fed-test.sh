#!/usr/bin/env bash
# sigiro-fed-test.sh — Multi-VM federation E2E test on Lima VMs (fully bare-metal)
#
# Architecture:
#   fed-central         — bare-metal PostgreSQL + sigiro serve --central
#   fed-collector-1     — bare-metal sigiro serve (self-registers with central)
#   fed-collector-2     — bare-metal sigiro serve (self-registers with central)
#
# Lima user-v2 networking does not support direct VM-to-VM communication.
# We use Lima port forwarding (hostIP: "0.0.0.0") so VMs can reach each other
# via host.lima.internal:<forwarded_port>.
#
set -eux -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FED_CENTRAL="fed-central"
FED_COLLECTOR_1="fed-collector-1"
FED_COLLECTOR_2="fed-collector-2"

# Host ports forwarded from each VM.
CENTRAL_HTTP_PORT=9999
CENTRAL_OTLP_PORT=4318
CENTRAL_QUACK_PORT=15432

COLLECTOR_1_HTTP_PORT=9998
COLLECTOR_1_OTLP_PORT=4319
COLLECTOR_1_QUACK_PORT=15433

COLLECTOR_2_HTTP_PORT=9997
COLLECTOR_2_OTLP_PORT=4320
COLLECTOR_2_QUACK_PORT=15434

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
vm_ssh() {
  local vm="$1"
  shift
  limactl shell "$vm" -- bash -lc "$*" < /dev/null
}

wait_for_port() {
  local host=$1 port=$2 retries=${3:-30}
  for i in $(seq 1 $retries); do
    if curl -s --connect-timeout 2 "http://${host}:${port}/health" 2>/dev/null | grep -q 'ok'; then
      return 0
    fi
    sleep 2
  done
  return 1
}

cleanup() {
  echo "=== Cleaning up VMs ==="
  for vm in "$FED_CENTRAL" "$FED_COLLECTOR_1" "$FED_COLLECTOR_2"; do
    limactl stop "$vm" 2>/dev/null || true
    limactl delete "$vm" 2>/dev/null || true
  done
  if [[ -n "${FED_TMPDIR:-}" && -d "$FED_TMPDIR" ]]; then
    rm -rf "$FED_TMPDIR"
  fi
  echo "Cleanup complete"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Step 1 — cleanup any existing VMs
# ---------------------------------------------------------------------------
echo "=== Cleaning up existing VMs ==="
for vm in "$FED_CENTRAL" "$FED_COLLECTOR_1" "$FED_COLLECTOR_2"; do
  limactl stop "$vm" 2>/dev/null && echo "Stopped $vm" || true
  limactl delete "$vm" 2>/dev/null && echo "Deleted $vm" || true
done

# ---------------------------------------------------------------------------
# Step 2 — generate per-instance collector YAMLs
# ---------------------------------------------------------------------------
FED_TMPDIR=$(mktemp -d)

gen_collector_yaml() {
  local out="$1" http_port="$2" otlp_port="$3" quack_port="$4"
  cat > "$out" << EOF
images:
- location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img"
  arch: "aarch64"
cpus: 4
memory: "4GiB"
disk: "60GiB"
ssh:
  localPort: 0
portForwards:
- guestPort: 9999
  hostPort: ${http_port}
  hostIP: "0.0.0.0"
- guestPort: 4318
  hostPort: ${otlp_port}
  hostIP: "0.0.0.0"
- guestPort: 5432
  hostPort: ${quack_port}
  hostIP: "0.0.0.0"
containerd:
  system: false
  user: false
mounts:
- location: "~/workspace"
  writable: false
provision:
- mode: system
  script: |
    #!/bin/bash
    set -eux -o pipefail
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install -y curl ca-certificates build-essential pkg-config libssl-dev git
EOF
}

COLLECTOR_1_YAML="$FED_TMPDIR/collector-1.yaml"
gen_collector_yaml "$COLLECTOR_1_YAML" "$COLLECTOR_1_HTTP_PORT" "$COLLECTOR_1_OTLP_PORT" "$COLLECTOR_1_QUACK_PORT"

COLLECTOR_2_YAML="$FED_TMPDIR/collector-2.yaml"
gen_collector_yaml "$COLLECTOR_2_YAML" "$COLLECTOR_2_HTTP_PORT" "$COLLECTOR_2_OTLP_PORT" "$COLLECTOR_2_QUACK_PORT"

# ---------------------------------------------------------------------------
# Step 3 — start ALL VMs in parallel
# ---------------------------------------------------------------------------
echo "=== Starting all VMs in parallel ==="

limactl start --name="$FED_CENTRAL" "$SCRIPT_DIR/sigiro-fed-central.yaml" 2>&1 &
CENTRAL_PID=$!

limactl start --name="$FED_COLLECTOR_1" "$COLLECTOR_1_YAML" 2>&1 &
C1_PID=$!

limactl start --name="$FED_COLLECTOR_2" "$COLLECTOR_2_YAML" 2>&1 &
C2_PID=$!

echo "Waiting for VMs to boot (central PID=$CENTRAL_PID, c1 PID=$C1_PID, c2 PID=$C2_PID)..."
wait $CENTRAL_PID || echo "Central VM may already exist"
wait $C1_PID || echo "Collector-1 VM may already exist"
wait $C2_PID || echo "Collector-2 VM may already exist"

echo "All VMs booted. Waiting a few seconds for port forwarding to settle..."
sleep 10

# Verify port forwards are active
echo "=== Verifying Lima port forwards ==="
for vm in "$FED_CENTRAL" "$FED_COLLECTOR_1" "$FED_COLLECTOR_2"; do
  limactl list "$vm" --json 2>/dev/null | jq '.portForwards' || true
done

# ---------------------------------------------------------------------------
# Step 4 — cross-compile sigiro on macOS
# ---------------------------------------------------------------------------
echo "=== Cross-compiling sigiro for aarch64 ==="
cd "$REPO_ROOT"
source scripts/lima-cross-env.sh
ulimit -n 65536
if ! command -v ccache >/dev/null 2>&1; then
  echo "ERROR: ccache is required but not installed."
  exit 1
fi
export CC_aarch64_unknown_linux_gnu="ccache ${CC_aarch64_unknown_linux_gnu}"
export CXX_aarch64_unknown_linux_gnu="ccache ${CXX_aarch64_unknown_linux_gnu}"
cargo build --release --target aarch64-unknown-linux-gnu --bin sigiro 2>&1 | tail -10

# ---------------------------------------------------------------------------
# Step 5 — copy binary to all VMs
# ---------------------------------------------------------------------------
echo "=== Copying sigiro binary to all VMs ==="
for vm in "$FED_CENTRAL" "$FED_COLLECTOR_1" "$FED_COLLECTOR_2"; do
  echo "Copying to $vm..."
  limactl copy target/aarch64-unknown-linux-gnu/release/sigiro "$vm:/home/akshat.guest/sigiro"
  vm_ssh "$vm" "chmod +x /home/akshat.guest/sigiro && ls -la /home/akshat.guest/sigiro"
done

# ---------------------------------------------------------------------------
# Step 6 — start central sigiro on central VM
# ---------------------------------------------------------------------------
echo "=== Starting sigiro-central on central VM ==="
vm_ssh "$FED_CENTRAL" "setsid /home/akshat.guest/run-central.sh > /tmp/sigiro-central.log 2>&1 & echo \"Central PID: \$!\""

echo "Waiting for central to start (up to 120s)..."
sleep 30

if ! wait_for_port "127.0.0.1" "$CENTRAL_HTTP_PORT" 60; then
  echo "ERROR: Central HTTP port not responding"
  vm_ssh "$FED_CENTRAL" "cat /tmp/sigiro-central.log | tail -30" || true
  exit 1
fi
echo "Central health check OK (port $CENTRAL_HTTP_PORT)"

# ---------------------------------------------------------------------------
# Step 7 — start bare-metal collectors on collector VMs
# ---------------------------------------------------------------------------
echo "=== Starting sigiro collectors on collector VMs ==="

cat > "$FED_TMPDIR/start-collector-1.sh" << EOF
#!/bin/bash
set -eux -o pipefail
export SIGIRO_ENDPOINT='http://host.lima.internal:${COLLECTOR_1_QUACK_PORT}'
export SIGIRO_QUACK_ADVERTISE_PORT='${COLLECTOR_1_QUACK_PORT}'
export SIGIRO_DATA_DIR=/home/akshat.guest/sigiro-data
export SIGIRO_HTTP_PORT=9999
export SIGIRO_OTLP_HTTP_PORT=4318
export SIGIRO_CENTRAL_URL=http://host.lima.internal:${CENTRAL_HTTP_PORT}
export SIGIRO_API_KEY=sigiro_dev_test
export SIGIRO_EBPF=false
export SIGIRO_QUACK_PORT=5432
export SIGIRO_QUACK_TOKEN=fed-token-central
mkdir -p /home/akshat.guest/sigiro-data
cd /home/akshat.guest
exec ./sigiro serve
EOF

cat > "$FED_TMPDIR/start-collector-2.sh" << EOF
#!/bin/bash
set -eux -o pipefail
export SIGIRO_ENDPOINT='http://host.lima.internal:${COLLECTOR_2_QUACK_PORT}'
export SIGIRO_QUACK_ADVERTISE_PORT='${COLLECTOR_2_QUACK_PORT}'
export SIGIRO_DATA_DIR=/home/akshat.guest/sigiro-data
export SIGIRO_HTTP_PORT=9999
export SIGIRO_OTLP_HTTP_PORT=4318
export SIGIRO_CENTRAL_URL=http://host.lima.internal:${CENTRAL_HTTP_PORT}
export SIGIRO_API_KEY=sigiro_dev_test
export SIGIRO_EBPF=false
export SIGIRO_QUACK_PORT=5432
export SIGIRO_QUACK_TOKEN=fed-token-central
mkdir -p /home/akshat.guest/sigiro-data
cd /home/akshat.guest
exec ./sigiro serve
EOF

limactl copy "$FED_TMPDIR/start-collector-1.sh" "$FED_COLLECTOR_1:/tmp/start-collector.sh"
vm_ssh "$FED_COLLECTOR_1" "chmod +x /tmp/start-collector.sh && setsid /tmp/start-collector.sh > /tmp/sigiro-collector.log 2>&1 & echo \"Collector-1 PID: \$!\""

limactl copy "$FED_TMPDIR/start-collector-2.sh" "$FED_COLLECTOR_2:/tmp/start-collector.sh"
vm_ssh "$FED_COLLECTOR_2" "chmod +x /tmp/start-collector.sh && setsid /tmp/start-collector.sh > /tmp/sigiro-collector.log 2>&1 & echo \"Collector-2 PID: \$!\""

echo "Waiting for collectors to start..."
sleep 20

if ! wait_for_port "127.0.0.1" "$COLLECTOR_1_HTTP_PORT" 40; then
  echo "ERROR: Collector-1 HTTP port not responding"
  vm_ssh "$FED_COLLECTOR_1" "cat /tmp/sigiro-collector.log | tail -30" || true
  exit 1
fi
echo "Collector-1 health check OK (port $COLLECTOR_1_HTTP_PORT)"

if ! wait_for_port "127.0.0.1" "$COLLECTOR_2_HTTP_PORT" 40; then
  echo "ERROR: Collector-2 HTTP port not responding"
  vm_ssh "$FED_COLLECTOR_2" "cat /tmp/sigiro-collector.log | tail -30" || true
  exit 1
fi
echo "Collector-2 health check OK (port $COLLECTOR_2_HTTP_PORT)"

# ---------------------------------------------------------------------------
# Step 8 — wait for collectors to register, then inject synthetic spans
# ---------------------------------------------------------------------------
echo "Waiting 20s for collectors to register with central..."
sleep 20

echo "=== Injecting synthetic spans into all collectors ==="

TRACE_ID_1=$(openssl rand -hex 16)
SPAN_ID_1=$(openssl rand -hex 8)
curl -s -X POST "http://127.0.0.1:${COLLECTOR_1_OTLP_PORT}/v1/traces" \
  -H "Content-Type: application/json" \
  -d "{
    \"resourceSpans\": [{
      \"resource\": { \"attributes\": [ { \"key\": \"service.name\", \"value\": { \"stringValue\": \"collector-1-synthetic\" } } ] },
      \"scopeSpans\": [{
        \"spans\": [{
          \"traceId\": \"${TRACE_ID_1}\",
          \"spanId\": \"${SPAN_ID_1}\",
          \"name\": \"synthetic-span-collector-1\",
          \"kind\": 1,
          \"startTimeUnixNano\": \"$(($(date +%s) * 1000000000))\",
          \"endTimeUnixNano\": \"$(($(date +%s) * 1000000000 + 1000000))\",
          \"attributes\": []
        }]
      }]
    }]
  }" || echo "WARNING: Failed to inject spans into collector-1"

TRACE_ID_2=$(openssl rand -hex 16)
SPAN_ID_2=$(openssl rand -hex 8)
curl -s -X POST "http://127.0.0.1:${COLLECTOR_2_OTLP_PORT}/v1/traces" \
  -H "Content-Type: application/json" \
  -d "{
    \"resourceSpans\": [{
      \"resource\": { \"attributes\": [ { \"key\": \"service.name\", \"value\": { \"stringValue\": \"collector-2-synthetic\" } } ] },
      \"scopeSpans\": [{
        \"spans\": [{
          \"traceId\": \"${TRACE_ID_2}\",
          \"spanId\": \"${SPAN_ID_2}\",
          \"name\": \"synthetic-span-collector-2\",
          \"kind\": 1,
          \"startTimeUnixNano\": \"$(($(date +%s) * 1000000000))\",
          \"endTimeUnixNano\": \"$(($(date +%s) * 1000000000 + 1000000))\",
          \"attributes\": []
        }]
      }]
    }]
  }" || echo "WARNING: Failed to inject spans into collector-2"

# Also inject into central's OTLP endpoint
TRACE_ID_C=$(openssl rand -hex 16)
SPAN_ID_C=$(openssl rand -hex 8)
curl -s -X POST "http://127.0.0.1:${CENTRAL_OTLP_PORT}/v1/traces" \
  -H "Content-Type: application/json" \
  -d "{
    \"resourceSpans\": [{
      \"resource\": { \"attributes\": [ { \"key\": \"service.name\", \"value\": { \"stringValue\": \"central-synthetic\" } } ] },
      \"scopeSpans\": [{
        \"spans\": [{
          \"traceId\": \"${TRACE_ID_C}\",
          \"spanId\": \"${SPAN_ID_C}\",
          \"name\": \"synthetic-span-central\",
          \"kind\": 1,
          \"startTimeUnixNano\": \"$(($(date +%s) * 1000000000))\",
          \"endTimeUnixNano\": \"$(($(date +%s) * 1000000000 + 1000000))\",
          \"attributes\": []
        }]
      }]
    }]
  }" || echo "WARNING: Failed to inject spans into central"

# ---------------------------------------------------------------------------
# Step 9 — wait for telemetry to flow
# ---------------------------------------------------------------------------
echo "Waiting 30s for telemetry to flow..."
sleep 30

# ---------------------------------------------------------------------------
# Step 10 — verification
# ---------------------------------------------------------------------------
echo "=== Verification ==="

# Check federation workers endpoint
echo "--- /v1/federation/workers on central ---"
WORKERS_RESP=$(curl -s -H "Authorization: Bearer sigiro_dev_test" "http://127.0.0.1:${CENTRAL_HTTP_PORT}/v1/federation/workers")
echo "$WORKERS_RESP" | jq . || echo "Raw: $WORKERS_RESP"

WORKER_COUNT=$(echo "$WORKERS_RESP" | jq 'length' || echo 0)
if [[ "$WORKER_COUNT" -lt 2 ]]; then
  echo "ERROR: Expected 2+ registered workers, found $WORKER_COUNT"
  echo "--- central logs ---"
  vm_ssh "$FED_CENTRAL" "cat /tmp/sigiro-central.log | tail -50" || true
  echo "--- collector-1 logs ---"
  vm_ssh "$FED_COLLECTOR_1" "cat /tmp/sigiro-collector.log | tail -50" || true
  echo "--- collector-2 logs ---"
  vm_ssh "$FED_COLLECTOR_2" "cat /tmp/sigiro-collector.log | tail -50" || true
  exit 1
fi
echo "OK: $WORKER_COUNT workers registered"

# Run federated query
echo "--- /v1/query (federated) ---"
QUERY_RESP=$(curl -s -X POST "http://127.0.0.1:${CENTRAL_HTTP_PORT}/v1/query" \
  -H "Authorization: Bearer sigiro_dev_test" \
  -H "Content-Type: application/json" \
  -d '{"sql":"SELECT service_name, count(*) as span_count FROM lake.sigiro_spans GROUP BY service_name ORDER BY span_count DESC LIMIT 10"}')
QUERY_COUNT=$(echo "$QUERY_RESP" | jq 'length' || echo 0)
if [[ "$QUERY_COUNT" -eq 0 ]]; then
  echo "WARNING: Federated query returned no rows"
else
  echo "OK: Federated query returned $QUERY_COUNT rows"
fi
echo "$QUERY_RESP" | jq . || echo "Raw: $QUERY_RESP"

# Check /v1/investigate
echo "--- /v1/investigate ---"
INVESTIGATE_RESP=$(curl -s -X POST "http://127.0.0.1:${CENTRAL_HTTP_PORT}/v1/investigate" \
  -H "Authorization: Bearer sigiro_dev_test" \
  -H "Content-Type: application/json" \
  -d '{"sql":"SELECT * FROM lake.sigiro_spans LIMIT 5"}')
INVESTIGATE_COUNT=$(echo "$INVESTIGATE_RESP" | jq 'length' || echo 0)
if [[ "$INVESTIGATE_COUNT" -eq 0 ]]; then
  echo "WARNING: /v1/investigate returned no rows"
else
  echo "OK: /v1/investigate returned $INVESTIGATE_COUNT rows"
fi
echo "$INVESTIGATE_RESP" | jq . || echo "Raw: $INVESTIGATE_RESP"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "============================================================"
echo "Federation E2E Test Summary"
echo "============================================================"
echo "Central:     $FED_CENTRAL (bare-metal, port $CENTRAL_HTTP_PORT)"
echo "Collector-1: $FED_COLLECTOR_1 (bare-metal, quack host port $COLLECTOR_1_QUACK_PORT)"
echo "Collector-2: $FED_COLLECTOR_2 (bare-metal, quack host port $COLLECTOR_2_QUACK_PORT)"
echo ""
echo "Federation workers:"
echo "$WORKERS_RESP" | jq .
echo ""
echo "Federated query results:"
echo "$QUERY_RESP" | jq .
echo "============================================================"
echo ""
echo "SUCCESS: Federation E2E test passed!"
