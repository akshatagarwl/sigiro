#!/usr/bin/env bash
# sigiro-selfhost-test.sh — Two-instance self-hosting test on Lima VMs
#
# Architecture:
#   sigiro-selfhost-old  — "backend" Sigiro (HTTP:9999, gRPC:4317, OTLP HTTP:4318)
#   sigiro-selfhost-new  — "self-hosted" Sigiro (HTTP:9989, gRPC:4316, OTLP HTTP:4315)
#                          obi instruments it and exports OTLP to old instance
#
set -eux -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

OLD_VM="sigiro-selfhost-old"
NEW_VM="sigiro-selfhost-new"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
vm_ssh() { limactl shell "$1" -- bash -lc "$2"; }
vm_cp() { limactl copy "$1" "$2"; }

get_vm_ip() {
  limactl list "$1" --json 2>/dev/null | \
    jq -r '.[] | select(.name=="'"$1"'") | .ssh_address' | \
    sed 's/127.0.0.1://' | head -1
}

wait_for_port() {
  local host=$1 port=$2 retries=${3:-30}
  for i in $(seq 1 $retries); do
    if curl -s --connect-timeout 2 "http://${host}:${port}/health" | grep -q '"ok"'; then
      return 0
    fi
    sleep 1
  done
  return 1
}

# ---------------------------------------------------------------------------
# Step 1 — create/start VMs
# ---------------------------------------------------------------------------
echo "=== Ensuring Lima VMs are running ==="

limactl start --name="$OLD_VM" "$SCRIPT_DIR/sigiro-selfhost-old.yaml" 2>&1 || \
  echo "VM $OLD_VM may already exist or started"

limactl start --name="$NEW_VM" "$SCRIPT_DIR/sigiro-selfhost-new.yaml" 2>&1 || \
  echo "VM $NEW_VM may already exist or started"

echo "Waiting for VMs to boot..."
sleep 10

# ---------------------------------------------------------------------------
# Step 2 — get IPs
# ---------------------------------------------------------------------------
OLD_IP=$(get_vm_ip "$OLD_VM")
NEW_IP=$(get_vm_ip "$NEW_VM")

if [[ -z "$OLD_IP" ]] || [[ -z "$NEW_IP" ]]; then
  echo "ERROR: Could not determine VM IPs"
  limactl list
  exit 1
fi

echo "Old VM: $OLD_VM @ $OLD_IP"
echo "New VM: $NEW_IP @ $NEW_VM"

# ---------------------------------------------------------------------------
# Step 3 — on OLD VM: clone repo and build natively
# ---------------------------------------------------------------------------
echo "=== [OLD] Setting up Sigiro from source ==="
vm_ssh "$OLD_VM" "which cargo || (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable && rustup target add aarch64-unknown-linux-gnu)"

# Clone and build on the old VM (uses the mounted workspace path)
vm_ssh "$OLD_VM" "rustup target add aarch64-unknown-linux-gnu 2>/dev/null || true"

# Build sigiro natively on the old VM using the workspace mount at /home/akshat.guest
vm_ssh "$OLD_VM" "cd /home/akshat.guest && cargo build --release 2>&1 | tail -5"

# ---------------------------------------------------------------------------
# Step 4 — on NEW VM: clone repo and build natively
# ---------------------------------------------------------------------------
echo "=== [NEW] Setting up Sigiro from source ==="
vm_ssh "$NEW_VM" "rustup target add aarch64-unknown-linux-gnu 2>/dev/null || true"
vm_ssh "$NEW_VM" "cd /home/akshat.guest && cargo build --release 2>&1 | tail -5"

# ---------------------------------------------------------------------------
# Step 5 — start OLD instance (backend) on default ports
# ---------------------------------------------------------------------------
echo "=== [OLD] Starting Sigiro backend on $OLD_IP ==="
vm_ssh "$OLD_VM" "pkill -f sigiro 2>/dev/null || true; rm -rf /tmp/sigiro-old; mkdir -p /tmp/sigiro-old"
vm_ssh "$OLD_VM" "cd /home/akshat.guest && \
  SIGIRO_DATA_DIR=/tmp/sigiro-old \
  SIGIRO_HTTP_PORT=9999 \
  SIGIRO_GRPC_PORT=4317 \
  SIGIRO_OTLP_HTTP_PORT=4318 \
  nohup ./target/release/sigiro serve --no-ebpf > /tmp/sigiro-old.log 2>&1 & \
  echo \"Old PID: \$!\""

sleep 5

if wait_for_port "$OLD_IP" 9999 30; then
  echo "[OLD] Health check OK on port 9999"
else
  echo "[OLD] WARNING: health check failed, checking logs"
  vm_ssh "$OLD_VM" "tail -30 /tmp/sigiro-old.log"
fi

# ---------------------------------------------------------------------------
# Step 6 — start NEW instance with obi pointing at OLD
# ---------------------------------------------------------------------------
echo "=== [NEW] Starting Sigiro self-hosted on $NEW_IP (pointing at $OLD_IP) ==="
vm_ssh "$NEW_VM" "pkill -f sigiro 2>/dev/null || true; rm -rf /tmp/sigiro-new; mkdir -p /tmp/sigiro-new"
vm_ssh "$NEW_VM" "cd /home/akshat.guest && \
  SIGIRO_DATA_DIR=/tmp/sigiro-new \
  SIGIRO_HTTP_PORT=9989 \
  SIGIRO_GRPC_PORT=4316 \
  SIGIRO_OTLP_HTTP_PORT=4315 \
  SIGIRO_OTEL_ENDPOINT=http://${OLD_IP}:4317 \
  SIGIRO_PYROSCOPE_ENDPOINT=http://${OLD_IP}:9999 \
  nohup ./target/release/sigiro serve --ebpf > /tmp/sigiro-new.log 2>&1 & \
  echo \"New PID: \$!\""

sleep 10

if wait_for_port "$NEW_IP" 9989 30; then
  echo "[NEW] Health check OK on port 9989"
else
  echo "[NEW] WARNING: health check failed, checking logs"
  vm_ssh "$NEW_VM" "tail -30 /tmp/sigiro-new.log"
fi

# ---------------------------------------------------------------------------
# Step 7 — query investigate endpoint on OLD
# ---------------------------------------------------------------------------
echo "=== Querying /investigate on OLD instance ==="
NOW=$(date +%s)
SINCE=$((NOW - 600))

INVESTIGATE_RESP=$(curl -s -X POST "http://${OLD_IP}:9999/investigate" \
  -H "Authorization: Bearer sigiro_dev_test" \
  -H "Content-Type: application/json" \
  -d "{\"service\":\"sigiro\",\"from\":$SINCE,\"now\":$NOW}")

echo "$INVESTIGATE_RESP" | jq . || echo "Raw response: $INVESTIGATE_RESP"

# ---------------------------------------------------------------------------
# Step 8 — collect logs
# ---------------------------------------------------------------------------
echo ""
echo "=== [OLD] Logs (last 40 lines) ==="
vm_ssh "$OLD_VM" "tail -40 /tmp/sigiro-old.log"

echo ""
echo "=== [NEW] Logs (last 40 lines) ==="
vm_ssh "$NEW_VM" "tail -40 /tmp/sigiro-new.log"

# ---------------------------------------------------------------------------
# Step 9 — if obi binary available, copy it to new VM
# ---------------------------------------------------------------------------
if [[ -f "$SCRIPT_DIR/obi-aarch64" ]]; then
  echo "=== Copying obi binary to new VM ==="
  vm_cp "$SCRIPT_DIR/obi-aarch64" "$NEW_VM:/home/akshat.guest/obi"
  vm_ssh "$NEW_VM" "chmod +x /home/akshat.guest/obi && ls -la /home/akshat.guest/obi"
fi
