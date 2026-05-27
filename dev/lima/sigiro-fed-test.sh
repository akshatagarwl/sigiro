#!/usr/bin/env bash
# sigiro-fed-test.sh — Multi-VM federation E2E test on Lima VMs
#
# Architecture:
#   fed-central         — k3s server + sigiro serve --central (HTTP :9999, OTLP :4318)
#   fed-collector-1     — k3s worker + sigiro serve --ebpf (self-registers with central)
#   fed-collector-2     — k3s worker + sigiro serve --ebpf (self-registers with central)
#
set -eux -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FED_CENTRAL="fed-central"
FED_COLLECTOR_1="fed-collector-1"
FED_COLLECTOR_2="fed-collector-2"

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
    if curl -s --connect-timeout 2 "http://${host}:${port}/health" 2>/dev/null | grep -q '"ok"'; then
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
# Step 2 — start central VM (k3s server)
# ---------------------------------------------------------------------------
echo "=== Starting central VM (k3s server) ==="
limactl start --name="$FED_CENTRAL" "$SCRIPT_DIR/sigiro-fed-central.yaml" 2>&1 || \
  echo "VM $FED_CENTRAL may already exist"

echo "Waiting for central VM to boot..."
sleep 30

CENTRAL_IP=$(get_vm_ip "$FED_CENTRAL")
if [[ -z "$CENTRAL_IP" ]]; then
  echo "ERROR: Could not determine central VM IP"
  limactl list
  exit 1
fi
echo "Central VM: $FED_CENTRAL @ $CENTRAL_IP"

# Wait for k3s to be ready on central
echo "Waiting for k3s API on central..."
for i in $(seq 1 60); do
  if vm_ssh "$FED_CENTRAL" sudo k3s kubectl get nodes &>/dev/null; then
    echo "k3s API is ready"
    break
  fi
  sleep 2
done

# ---------------------------------------------------------------------------
# Step 3 — get k3s agent token and server IP
# ---------------------------------------------------------------------------
echo "=== Getting k3s agent token ==="
K3S_TOKEN=$(vm_ssh "$FED_CENTRAL" sudo cat /var/lib/rancher/k3s/server/node-token)
if [[ -z "$K3S_TOKEN" ]]; then
  echo "ERROR: Could not read k3s agent token"
  exit 1
fi
echo "Token obtained (length: ${#K3S_TOKEN})"

# ---------------------------------------------------------------------------
# Step 4 — start collector VMs (k3s workers)
# ---------------------------------------------------------------------------
echo "=== Starting collector-1 VM ==="
limactl start --name="$FED_COLLECTOR_1" \
  --env K3S_SERVER_IP="$CENTRAL_IP" \
  --env K3S_AGENT_TOKEN="$K3S_TOKEN" \
  "$SCRIPT_DIR/sigiro-fed-collector.yaml" 2>&1 || \
  echo "VM $FED_COLLECTOR_1 may already exist"

echo "=== Starting collector-2 VM ==="
limactl start --name="$FED_COLLECTOR_2" \
  --env K3S_SERVER_IP="$CENTRAL_IP" \
  --env K3S_AGENT_TOKEN="$K3S_TOKEN" \
  "$SCRIPT_DIR/sigiro-fed-collector.yaml" 2>&1 || \
  echo "VM $FED_COLLECTOR_2 may already exist"

echo "Waiting for collector VMs to boot..."
sleep 20

COLLECTOR_1_IP=$(get_vm_ip "$FED_COLLECTOR_1")
COLLECTOR_2_IP=$(get_vm_ip "$FED_COLLECTOR_2")
echo "Collector-1: $FED_COLLECTOR_1 @ $COLLECTOR_1_IP"
echo "Collector-2: $FED_COLLECTOR_2 @ $COLLECTOR_2_IP"

# ---------------------------------------------------------------------------
# Step 5 — cross-compile sigiro on macOS
# ---------------------------------------------------------------------------
echo "=== Cross-compiling sigiro for aarch64 ==="
cd "$REPO_ROOT"
export AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar
ulimit -n 65536
cargo zigbuild --release --target aarch64-unknown-linux-gnu 2>&1 | tail -10

# ---------------------------------------------------------------------------
# Step 6 — copy binary to all VMs
# ---------------------------------------------------------------------------
echo "=== Copying sigiro binary to all VMs ==="
for vm in "$FED_CENTRAL" "$FED_COLLECTOR_1" "$FED_COLLECTOR_2"; do
  echo "Copying to $vm..."
  limactl copy target/aarch64-unknown-linux-gnu/release/sigiro "$vm:/home/akshat.guest/sigiro"
  vm_ssh "$vm" "chmod +x /home/akshat.guest/sigiro && ls -la /home/akshat.guest/sigiro"
done

# ---------------------------------------------------------------------------
# Step 7 — build Docker image and load into k3s
# ---------------------------------------------------------------------------
echo "=== Building Docker image ==="
docker build -t sigiro-collector:local -f "$REPO_ROOT/Dockerfile" "$REPO_ROOT" 2>&1 | tail -10

echo "=== Loading Docker image into k3s ==="
docker save sigiro-collector:local | vm_ssh "$FED_CENTRAL" sudo k3s ctr images import -

# ---------------------------------------------------------------------------
# Step 8 — deploy k8s manifests
# ---------------------------------------------------------------------------
echo "=== Deploying k8s manifests ==="

kubectl() {
  vm_ssh "$FED_CENTRAL" sudo k3s kubectl --kubeconfig /etc/rancher/k3s/k3s.yaml "$@"
}

kubectl apply -f "$REPO_ROOT/tests/k8s/federation/namespace.yaml"
kubectl apply -f "$REPO_ROOT/tests/k8s/federation/postgresql.yaml"
kubectl apply -f "$REPO_ROOT/tests/k8s/federation/minio.yaml"

echo "Waiting for postgresql and minio to be ready..."
kubectl -n sigiro-fed wait --for=condition=ready pod -l app=postgresql --timeout=120s 2>/dev/null || true
kubectl -n sigiro-fed wait --for=condition=ready pod -l app=minio --timeout=120s 2>/dev/null || true

kubectl apply -f "$REPO_ROOT/tests/k8s/federation/sigiro-central.yaml"
echo "Waiting for sigiro-central pod..."
kubectl -n sigiro-fed wait --for=condition=ready pod -l app=sigiro-central --timeout=180s

kubectl apply -f "$REPO_ROOT/tests/k8s/federation/sigiro-collector-daemonset.yaml"
echo "Waiting for sigiro-collector pods..."
kubectl -n sigiro-fed wait --for=condition=ready pod -l app=sigiro-collector --timeout=180s

# ---------------------------------------------------------------------------
# Step 9 — wait for demo services
# ---------------------------------------------------------------------------
if [[ -f "$REPO_ROOT/tests/k8s/federation/demo-services.yaml" ]]; then
  echo "=== Deploying demo services ==="
  kubectl apply -f "$REPO_ROOT/tests/k8s/federation/demo-services.yaml"
  echo "Waiting for frontend to be ready..."
  kubectl -n sigiro-fed wait --for=condition=ready deployment frontend --timeout=180s 2>/dev/null || true
fi

# ---------------------------------------------------------------------------
# Step 10 — verification
# ---------------------------------------------------------------------------
echo "=== Verification ==="

# Check k8s nodes
echo "--- k8s nodes ---"
kubectl get nodes

# Check all pods
echo "--- pods in sigiro-fed ---"
kubectl get pods -n sigiro-fed

# Wait for telemetry to flow
echo "Waiting 60s for telemetry to flow..."
sleep 60

# Check federation workers endpoint
echo "--- /v1/federation/workers on central ---"
CENTRAL_POD=$(kubectl get pods -n sigiro-fed -l app=sigiro-central -o jsonpath='{.items[0].metadata.name}')
WORKERS_RESP=$(kubectl exec -n sigiro-fed "$CENTRAL_POD" -- curl -s -H "Authorization: Bearer sigiro_dev_test" "http://localhost:9999/v1/federation/workers")
echo "$WORKERS_RESP" | jq . || echo "Raw: $WORKERS_RESP"

# Check investigate endpoint
echo "--- /v1/investigate on central ---"
NOW=$(date +%s)
SINCE=$((NOW - 600))
INVESTIGATE_RESP=$(kubectl exec -n sigiro-fed "$CENTRAL_POD" -- \
  curl -s -X POST "http://localhost:9999/v1/investigate" \
    -H "Authorization: Bearer sigiro_dev_test" \
    -H "Content-Type: application/json" \
    -d "{\"service\":\"sigiro\",\"from\":$SINCE,\"now\":$NOW}")
echo "$INVESTIGATE_RESP" | jq . || echo "Raw: $INVESTIGATE_RESP"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "============================================================"
echo "Federation E2E Test Summary"
echo "============================================================"
echo "Central IP:  $CENTRAL_IP"
echo "Collector-1: $COLLECTOR_1_IP"
echo "Collector-2: $COLLECTOR_2_IP"
echo ""
echo "k8s nodes:"
kubectl get nodes -o wide
echo ""
echo "pods:"
kubectl get pods -n sigiro-fed -o wide
echo ""
echo "Federation workers:"
echo "$WORKERS_RESP" | jq .
echo "============================================================"
