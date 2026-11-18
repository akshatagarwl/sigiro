#!/usr/bin/env bash
# sigiro-federation-test.sh — Multi-node federation test on Lima VMs
#
# Architecture (vzNAT):
#   - Central VM forwards guest:9999 → host:10000 (0.0.0.0)
#   - Worker VM forwards guest:9999 → host:10001, guest:4318 → host:10102 (0.0.0.0)
#   - VMs reach each other via host.lima.internal:<host_port>
#
set -eux -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

CENTRAL_VM="sigiro-fed-central"
WORKER_VM="sigiro-fed-worker"

# Host-side ports (forwarded from VMs)
CENTRAL_HOST_PORT=10000
WORKER_HTTP_HOST_PORT=10001
WORKER_OTLP_HOST_PORT=10102

# Shared PostgreSQL catalog runs in central VM, exposed via host relay.
# Both VMs connect through host.lima.internal:15432 → central:5432.
CATALOG_URL="postgresql://sigiro:sigiro@host.lima.internal:15432/sigiro"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
vm_ssh() { limactl shell "$1" bash -lc "$2"; }
vm_cp() { limactl copy "$1" "$2"; }

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
# Step 1 — Linux ARM64 sigiro binary (host cross-compile; see dev/lima/README.md)
# ---------------------------------------------------------------------------
cd "$PROJECT_ROOT"
# shellcheck source=scripts/lima-cross-env.sh
source "${PROJECT_ROOT}/scripts/lima-cross-env.sh"

BINARY="${PROJECT_ROOT}/${SIGIRO_LIMA_SIGIRO_BIN}"
SPICED_HOST="${PROJECT_ROOT}/${SIGIRO_LIMA_SPICED_BIN}"

if [[ ! -f "$BINARY" ]]; then
  echo "=== [HOST] Building sigiro for ${SIGIRO_LIMA_TARGET} ==="
  ulimit -n 65536 2>/dev/null || true
  cargo build --release --target "${SIGIRO_LIMA_TARGET}" --bin sigiro
fi
file "$BINARY"
if [[ ! -f "$BINARY" ]]; then
  echo "ERROR: Missing $BINARY — run: just lima-build"
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
# Step 3 — spiced on central VM (host-built Linux ARM64 binary by default)
# ---------------------------------------------------------------------------
echo "=== [CENTRAL] Preparing spiced ==="
SPICED_VM_INSTALL="/usr/local/bin/spiced"

if [[ "${SKIP_SPICED:-}" == "1" ]]; then
  echo "SKIP_SPICED=1 — skipping spiced (control-plane only)"
elif [[ -f "$SPICED_HOST" ]]; then
  echo "Using host-built spiced: $SPICED_HOST"
  vm_cp "$SPICED_HOST" "${CENTRAL_VM}:${SPICED_VM_INSTALL}"
  vm_ssh "$CENTRAL_VM" "chmod +x ${SPICED_VM_INSTALL}"
  vm_ssh "$CENTRAL_VM" "${SPICED_VM_INSTALL} --version || true"
elif [[ "${FED_BUILD_SPICED_IN_VM:-}" == "1" ]]; then
  echo "FED_BUILD_SPICED_IN_VM=1 — building spiced inside VM (slow; prefer: just spiced-lima)"
  SPICED_VM_PATH="/home/akshat.guest/.spice/bin/spiced"
  vm_ssh "$CENTRAL_VM" "sudo apt-get update && sudo apt-get install -y build-essential pkg-config libssl-dev git clang lld protobuf-compiler"
  if ! vm_ssh "$CENTRAL_VM" "which rustc" >/dev/null 2>&1; then
    vm_ssh "$CENTRAL_VM" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable"
  fi
  vm_ssh "$CENTRAL_VM" "sudo fallocate -l 2G /swapfile 2>/dev/null || sudo dd if=/dev/zero of=/swapfile bs=1M count=2048; sudo chmod 600 /swapfile; sudo mkswap /swapfile; sudo swapon /swapfile || true"
  limactl copy -r "${PROJECT_ROOT}/third_party/spiceai" "${CENTRAL_VM}:/home/akshat.guest/sigiro-third_party-spiceai" 2>/dev/null || \
    vm_ssh "$CENTRAL_VM" "test -d /home/akshat.guest/sigiro/third_party/spiceai" || \
    echo "WARN: copy spiceai submodule to VM manually if in-VM build fails"
  vm_ssh "$CENTRAL_VM" "source \$HOME/.cargo/env && cd /home/akshat.guest/sigiro/third_party/spiceai 2>/dev/null || cd /home/akshat.guest/sigiro-third_party-spiceai && CARGO_BUILD_JOBS=1 SPICED_CUSTOM_FEATURES='duckdb,alloc-snmalloc' make install"
  vm_cp "$SPICED_VM_PATH" "${CENTRAL_VM}:${SPICED_VM_INSTALL}" 2>/dev/null || \
    vm_ssh "$CENTRAL_VM" "sudo cp -f $SPICED_VM_PATH ${SPICED_VM_INSTALL} && sudo chmod +x ${SPICED_VM_INSTALL}"
else
  echo "ERROR: Missing host spiced binary: $SPICED_HOST"
  echo "  Build on host:  just spiced-lima"
  echo "  Or slow VM build: FED_BUILD_SPICED_IN_VM=1 $0"
  echo "  Or skip spiced:   SKIP_SPICED=1 $0"
  exit 1
fi

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
echo "=== [CENTRAL] Waiting for PostgreSQL catalog ==="
for i in $(seq 1 30); do
  if vm_ssh "$CENTRAL_VM" "pg_isready -U sigiro -d sigiro >/dev/null 2>&1"; then
    echo "PostgreSQL catalog ready"
    break
  fi
  sleep 1
done

echo "=== [CENTRAL] Starting sigiro central ==="
vm_ssh "$CENTRAL_VM" "pkill -9 sigiro 2>/dev/null || true; rm -rf /tmp/sigiro-central; mkdir -p /tmp/sigiro-central"
vm_ssh "$CENTRAL_VM" "SIGIRO_DATA_DIR=/tmp/sigiro-central \
  SIGIRO_HTTP_PORT=9999 \
  SIGIRO_GRPC_PORT=4317 \
  SIGIRO_OTLP_HTTP_PORT=4318 \
  SIGIRO_CENTRAL=true \
  SIGIRO_CATALOG_URL=${CATALOG_URL} \
  HOSTNAME=sigiro-fed-central \
  SIGIRO_API_KEY=sigiro_dev_fed \
  setsid /home/akshat.guest/sigiro serve > /tmp/sigiro-central.log 2>&1 & \
  echo \"Central PID: \$!\""

sleep 5

if wait_for_health "localhost" "$CENTRAL_HOST_PORT" 30; then
  echo "[CENTRAL] Health check OK on port $CENTRAL_HOST_PORT"
else
  echo "[CENTRAL] WARNING: health check failed, checking logs"
  vm_ssh "$CENTRAL_VM" "tail -40 /tmp/sigiro-central.log"
  exit 1
fi

# ---------------------------------------------------------------------------
# Step 6 — start worker node
# ---------------------------------------------------------------------------
echo "=== [WORKER] Starting sigiro worker (central=host.lima.internal:$CENTRAL_HOST_PORT) ==="
vm_ssh "$WORKER_VM" "pkill -9 sigiro 2>/dev/null || true; rm -rf /tmp/sigiro-worker; mkdir -p /tmp/sigiro-worker"
vm_ssh "$WORKER_VM" "SIGIRO_DATA_DIR=/tmp/sigiro-worker \
  SIGIRO_HTTP_PORT=9999 \
  SIGIRO_GRPC_PORT=4317 \
  SIGIRO_OTLP_HTTP_PORT=4318 \
  SIGIRO_ENDPOINT=http://host.lima.internal:${WORKER_HTTP_HOST_PORT} \
  SIGIRO_CENTRAL_URL=http://host.lima.internal:${CENTRAL_HOST_PORT} \
  SIGIRO_CATALOG_URL=${CATALOG_URL} \
  HOSTNAME=sigiro-fed-worker \
  SIGIRO_API_KEY=sigiro_dev_fed \
  setsid /home/akshat.guest/sigiro serve > /tmp/sigiro-worker.log 2>&1 & \
  echo \"Worker PID: \$!\""

sleep 5

if wait_for_health "localhost" "$WORKER_HTTP_HOST_PORT" 30; then
  echo "[WORKER] Health check OK on port $WORKER_HTTP_HOST_PORT"
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

WORKERS_RESP=$(curl -s --connect-timeout 5 "http://localhost:${CENTRAL_HOST_PORT}/v1/federation/workers" \
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

if [[ "$WORKER_ENDPOINT" != "http://host.lima.internal:${WORKER_HTTP_HOST_PORT}" ]]; then
  echo "[FAIL] Worker endpoint mismatch: expected http://host.lima.internal:${WORKER_HTTP_HOST_PORT}, got $WORKER_ENDPOINT"
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

curl -s -X POST "http://localhost:${WORKER_OTLP_HOST_PORT}/v1/traces" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer sigiro_dev_fed" \
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
QUERY_RESP=$(curl -s --connect-timeout 5 "http://localhost:${WORKER_HTTP_HOST_PORT}/v1/query" \
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
# Step 11 — verify spiced is running on central
# ---------------------------------------------------------------------------
echo "=== [TEST] Verifying spiced process on central ==="
if vm_ssh "$CENTRAL_VM" "pgrep -x spiced > /dev/null"; then
  echo "[PASS] spiced process is running on central"
else
  echo "[WARN] spiced process not found on central (check logs)"
  vm_ssh "$CENTRAL_VM" "tail -20 /tmp/sigiro-central.log"
fi

# ---------------------------------------------------------------------------
# Step 12 — logs summary
# ---------------------------------------------------------------------------
echo ""
echo "=== [CENTRAL] Logs (last 30 lines) ==="
vm_ssh "$CENTRAL_VM" "tail -30 /tmp/sigiro-central.log"

echo ""
echo "=== [WORKER] Logs (last 30 lines) ==="
vm_ssh "$WORKER_VM" "tail -30 /tmp/sigiro-worker.log"

echo ""
echo "=== Federation Test Complete ==="
echo "Central:     http://localhost:${CENTRAL_HOST_PORT}"
echo "Worker:      http://localhost:${WORKER_HTTP_HOST_PORT}"
echo "Worker OTLP: http://localhost:${WORKER_OTLP_HOST_PORT}"
echo "Catalog:     postgresql://localhost:15432/sigiro (in central VM)"
