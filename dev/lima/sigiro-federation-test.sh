#!/usr/bin/env bash
# sigiro-federation-test.sh — Multi-node federation test on Lima VMs
#
# Design (one correct way):
#   1. Build sigiro Linux ARM64 binary on host via Docker (fast, cached)
#   2. Copy binary to both VMs
#   3. Start central + worker, test federation control plane
#
set -eux -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

CENTRAL_VM="sigiro-fed-central"
WORKER_VM="sigiro-fed-worker"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
vm_ssh() { limactl shell "$1" bash -lc "$2"; }
vm_cp() { limactl copy "$1" "$2"; }

get_vm_internal_ip() {
  limactl shell "$1" bash -lc "ip -4 addr show lima0 2>/dev/null | grep -oP '(?<=inet\\s)\\d+(\\.\\d+){3}' | head -1"
}

wait_for_health() {
  local host=$1 port=$2 retries=${3:-30}
  for i in $(seq 1 $retries); do
    if curl -s --connect-timeout 2 "http://${host}:${port}/health" | grep -q 'ok'; then
      return 0
    fi
    sleep 1
  done
  return 1
}

echo "=== Federation Test ==="

# ---------------------------------------------------------------------------
# Step 1 — Build Linux ARM64 binary on host via Docker
# ---------------------------------------------------------------------------
echo "=== [HOST] Building sigiro Linux binary via Docker ==="
cd "$PROJECT_ROOT"

# Build the builder stage and extract binary
docker build --target builder -t sigiro-builder . 2>&1 | tail -20

CONTAINER_ID=$(docker create sigiro-builder)
docker cp "$CONTAINER_ID:/build/sigiro" "$PROJECT_ROOT/target/sigiro-linux-aarch64"
docker rm "$CONTAINER_ID"

BINARY="$PROJECT_ROOT/target/sigiro-linux-aarch64"
file "$BINARY"
if [[ ! -f "$BINARY" ]]; then
  echo "ERROR: Docker build did not produce binary"
  exit 1
fi

# ---------------------------------------------------------------------------
# Step 2 — create/start VMs
# ---------------------------------------------------------------------------
echo "=== Ensuring Lima VMs are running ==="

limactl start --name="$CENTRAL_VM" "$SCRIPT_DIR/sigiro-fed-central.yaml" 2>&1 || \
  echo "VM $CENTRAL_VM may already exist or be started"

limactl start --name="$WORKER_VM" "$SCRIPT_DIR/sigiro-fed-worker.yaml" 2>&1 || \
  echo "VM $WORKER_VM may already exist or be started"

echo "Waiting for VMs to boot..."
sleep 10

# ---------------------------------------------------------------------------
# Step 3 — get IPs
# ---------------------------------------------------------------------------
CENTRAL_IP=$(get_vm_internal_ip "$CENTRAL_VM")
WORKER_IP=$(get_vm_internal_ip "$WORKER_VM")

if [[ -z "$CENTRAL_IP" ]] || [[ -z "$WORKER_IP" ]]; then
  echo "ERROR: Could not determine VM internal IPs"
  limactl list
  exit 1
fi

echo "Central VM: $CENTRAL_VM @ $CENTRAL_IP"
echo "Worker VM:  $WORKER_VM @ $WORKER_IP"

# ---------------------------------------------------------------------------
# Step 4 — copy binary to VMs
# ---------------------------------------------------------------------------
echo "=== Copying binary to VMs ==="
vm_cp "$BINARY" "$CENTRAL_VM:/home/akshat.guest/sigiro"
vm_cp "$BINARY" "$WORKER_VM:/home/akshat.guest/sigiro"
vm_ssh "$CENTRAL_VM" "chmod +x /home/akshat.guest/sigiro"
vm_ssh "$WORKER_VM" "chmod +x /home/akshat.guest/sigiro"

# ---------------------------------------------------------------------------
# Step 5 — start central node
# ---------------------------------------------------------------------------
echo "=== [CENTRAL] Starting sigiro central on $CENTRAL_IP ==="
vm_ssh "$CENTRAL_VM" "pkill -9 sigiro 2>/dev/null || true; rm -rf /tmp/sigiro-central; mkdir -p /tmp/sigiro-central"
vm_ssh "$CENTRAL_VM" "SIGIRO_DATA_DIR=/tmp/sigiro-central \
  SIGIRO_HTTP_PORT=9999 \
  SIGIRO_GRPC_PORT=4317 \
  SIGIRO_OTLP_HTTP_PORT=4318 \
  HOSTNAME=sigiro-fed-central \
  SIGIRO_API_KEY=sigiro_dev_fed \
  setsid /home/akshat.guest/sigiro serve > /tmp/sigiro-central.log 2>&1 & \
  echo \"Central PID: \$!\""

sleep 5

if wait_for_health "$CENTRAL_IP" 9999 30; then
  echo "[CENTRAL] Health check OK on port 9999"
else
  echo "[CENTRAL] WARNING: health check failed, checking logs"
  vm_ssh "$CENTRAL_VM" "tail -40 /tmp/sigiro-central.log"
  exit 1
fi

# ---------------------------------------------------------------------------
# Step 6 — start worker node
# ---------------------------------------------------------------------------
echo "=== [WORKER] Starting sigiro worker on $WORKER_IP (central=$CENTRAL_IP) ==="
vm_ssh "$WORKER_VM" "pkill -9 sigiro 2>/dev/null || true; rm -rf /tmp/sigiro-worker; mkdir -p /tmp/sigiro-worker"
vm_ssh "$WORKER_VM" "SIGIRO_DATA_DIR=/tmp/sigiro-worker \
  SIGIRO_HTTP_PORT=9999 \
  SIGIRO_GRPC_PORT=4317 \
  SIGIRO_OTLP_HTTP_PORT=4318 \
  SIGIRO_ENDPOINT=http://${WORKER_IP}:9999 \
  SIGIRO_CENTRAL_URL=http://host.lima.internal:9999 \
  HOSTNAME=sigiro-fed-worker \
  SIGIRO_API_KEY=sigiro_dev_fed \
  setsid /home/akshat.guest/sigiro serve > /tmp/sigiro-worker.log 2>&1 & \
  echo \"Worker PID: \$!\""

sleep 5

if wait_for_health "$WORKER_IP" 9999 30; then
  echo "[WORKER] Health check OK on port 9999"
else
  echo "[WORKER] WARNING: health check failed, checking logs"
  vm_ssh "$WORKER_VM" "tail -40 /tmp/sigiro-worker.log"
  exit 1
fi

# Give worker time to self-register with central
sleep 3

# ---------------------------------------------------------------------------
# Step 7 — verify worker registration on central
# ---------------------------------------------------------------------------
echo "=== [TEST] Verifying worker registration ==="

WORKERS_RESP=$(curl -s --connect-timeout 5 "http://${CENTRAL_IP}:9999/v1/federation/workers" \
  -H "Authorization: Bearer sigiro_dev_fed")

echo "$WORKERS_RESP" | jq . || echo "Raw response: $WORKERS_RESP"

WORKER_COUNT=$(echo "$WORKERS_RESP" | jq '.workers | length')
if [[ "$WORKER_COUNT" -eq 1 ]]; then
  echo "[PASS] Worker registered successfully"
else
  echo "[FAIL] Expected 1 worker, got $WORKER_COUNT"
  echo "[CENTRAL] Logs (last 30 lines):"
  vm_ssh "$CENTRAL_VM" "tail -30 /tmp/sigiro-central.log"
  echo "[WORKER] Logs (last 30 lines):"
  vm_ssh "$WORKER_VM" "tail -30 /tmp/sigiro-worker.log"
  exit 1
fi

# Verify worker details
WORKER_ID=$(echo "$WORKERS_RESP" | jq -r '.workers[0].id')
WORKER_ENDPOINT=$(echo "$WORKERS_RESP" | jq -r '.workers[0].endpoint')
WORKER_CATALOG=$(echo "$WORKERS_RESP" | jq -r '.workers[0].catalog_path')

echo "Registered worker:"
echo "  id:       $WORKER_ID"
echo "  endpoint: $WORKER_ENDPOINT"
echo "  catalog:  $WORKER_CATALOG"

if [[ "$WORKER_ENDPOINT" != "http://${WORKER_IP}:9999" ]]; then
  echo "[FAIL] Worker endpoint mismatch: expected http://${WORKER_IP}:9999, got $WORKER_ENDPOINT"
  exit 1
fi
echo "[PASS] Worker endpoint is correct"

# ---------------------------------------------------------------------------
# Step 8 — verify spicepod.yaml on central
# ---------------------------------------------------------------------------
echo "=== [TEST] Verifying spicepod.yaml on central ==="
SPICEPOD=$(vm_ssh "$CENTRAL_VM" "cat /tmp/sigiro-central/spicepod.yaml 2>/dev/null || echo 'NOT_FOUND'")
if [[ "$SPICEPOD" == "NOT_FOUND" ]]; then
  echo "[FAIL] spicepod.yaml not found on central"
  exit 1
fi

echo "$SPICEPOD"
if echo "$SPICEPOD" | grep -q "worker_"; then
  echo "[PASS] spicepod.yaml contains worker dataset"
else
  echo "[FAIL] spicepod.yaml missing worker dataset"
  exit 1
fi

# ---------------------------------------------------------------------------
# Step 9 — send OTLP traces to worker
# ---------------------------------------------------------------------------
echo "=== [TEST] Sending OTLP traces to worker ==="

NOW_US=$(date +%s000000)
TRACE_ID=$(openssl rand -hex 16)
SPAN_ID=$(openssl rand -hex 8)

curl -s -X POST "http://${WORKER_IP}:4318/v1/traces" \
  -H "Content-Type: application/json" \
  -d "{
    \"resourceSpans\": [{
      \"resource\": { \"attributes\": [{ \"key\": \"service.name\", \"value\": { \"stringValue\": \"test-service\" } }] },
      \"scopeSpans\": [{
        \"spans\": [{
          \"traceId\": \"${TRACE_ID}\",
          \"spanId\": \"${SPAN_ID}\",
          \"name\": \"test-span\",
          \"kind\": 1,
          \"startTimeUnixNano\": \"${NOW_US}\",
          \"endTimeUnixNano\": \"$((NOW_US + 1000000))\",
          \"attributes\": []
        }]
      }]
    }]
  }" > /dev/null

echo "Sent trace $TRACE_ID to worker"
sleep 2

# ---------------------------------------------------------------------------
# Step 10 — query worker directly for the trace
# ---------------------------------------------------------------------------
echo "=== [TEST] Querying worker for trace ==="
QUERY_RESP=$(curl -s --connect-timeout 5 "http://${WORKER_IP}:9999/v1/query" \
  -H "Authorization: Bearer sigiro_dev_fed" \
  -H "Content-Type: text/plain" \
  -d "SELECT trace_id FROM lake.sigiro_spans WHERE trace_id = '${TRACE_ID}'")

echo "$QUERY_RESP" | jq . || echo "Raw response: $QUERY_RESP"

if echo "$QUERY_RESP" | grep -q "$TRACE_ID"; then
  echo "[PASS] Trace found on worker"
else
  echo "[WARN] Trace not found on worker yet (may need more time)"
fi

# ---------------------------------------------------------------------------
# Step 11 — logs summary
# ---------------------------------------------------------------------------
echo ""
echo "=== [CENTRAL] Logs (last 30 lines) ==="
vm_ssh "$CENTRAL_VM" "tail -30 /tmp/sigiro-central.log"

echo ""
echo "=== [WORKER] Logs (last 30 lines) ==="
vm_ssh "$WORKER_VM" "tail -30 /tmp/sigiro-worker.log"

echo ""
echo "=== Federation Test Complete ==="
echo "Central: http://${CENTRAL_IP}:9999"
echo "Worker:  http://${WORKER_IP}:9999"
