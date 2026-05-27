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
vm_ssh() {
  local vm="$1"
  shift
  # Redirect stdin from /dev/null so limactl shell doesn't consume the
  # parent script's stdin when called inside $(...) command substitutions.
  limactl shell "$vm" -- bash -lc "$*" < /dev/null
}
vm_cp() { limactl copy "$1" "$2"; }

get_vm_ip() {
  # Get the VM's internal Lima network IP (eth0) so other VMs can reach it
  limactl shell "$1" -- ip -4 addr show eth0 2>/dev/null | \
    grep 'inet ' | awk '{print $2}' | cut -d/ -f1 | head -1
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
# Lima does not support --env on limactl start, so we generate per-instance
# YAMLs with the k3s values embedded.
FED_TMPDIR=$(mktemp -d)

gen_collector_yaml() {
  local out="$1"
  sed -e "s|\\\${K3S_SERVER_IP}|$CENTRAL_IP|g" \
      -e "s|\\\${K3S_AGENT_TOKEN}|$K3S_TOKEN|g" \
      "$SCRIPT_DIR/sigiro-fed-collector.yaml" > "$out"
}

echo "=== Starting collector-1 VM ==="
COLLECTOR_1_YAML="$FED_TMPDIR/collector-1.yaml"
gen_collector_yaml "$COLLECTOR_1_YAML"
limactl start --name="$FED_COLLECTOR_1" "$COLLECTOR_1_YAML" 2>&1 || \
  echo "VM $FED_COLLECTOR_1 may already exist"

echo "=== Starting collector-2 VM ==="
COLLECTOR_2_YAML="$FED_TMPDIR/collector-2.yaml"
gen_collector_yaml "$COLLECTOR_2_YAML"
limactl start --name="$FED_COLLECTOR_2" "$COLLECTOR_2_YAML" 2>&1 || \
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
source scripts/lima-cross-env.sh
ulimit -n 65536
# ccache is required for DuckDB C++ compilation — without it, every rebuild
# recompiles 500+ C++ files from scratch (~8-10 minutes).
if ! command -v ccache >/dev/null 2>&1; then
  echo "============================================================"
  echo "ERROR: ccache is required but not installed."
  echo ""
  echo "DuckDB's C++ compilation takes ~8-10 minutes without ccache."
  echo "Install it now:"
  echo "  macOS:   brew install ccache"
  echo "  Debian:  apt-get install ccache"
  echo "  Fedora:  dnf install ccache"
  echo "============================================================"
  exit 1
fi
export CC="ccache ${CC_aarch64_unknown_linux_gnu}"
export CXX="ccache ${CXX_aarch64_unknown_linux_gnu}"
export CC_aarch64_unknown_linux_gnu="$CC"
export CXX_aarch64_unknown_linux_gnu="$CXX"
cargo build --release --target aarch64-unknown-linux-gnu --bin sigiro 2>&1 | tail -10

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
IMG_TAR="$FED_TMPDIR/sigiro-collector.tar"
docker save sigiro-collector:local -o "$IMG_TAR"
limactl copy "$IMG_TAR" "$FED_CENTRAL:/tmp/sigiro-collector.tar"
vm_ssh "$FED_CENTRAL" "sudo k3s ctr -n k8s.io images import /tmp/sigiro-collector.tar"

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
if ! kubectl -n sigiro-fed wait --for=condition=ready pod -l app=sigiro-central --timeout=180s; then
  echo "ERROR: sigiro-central pod did not become ready"
  kubectl -n sigiro-fed get pods -o wide
  kubectl -n sigiro-fed describe pod -l app=sigiro-central
  kubectl -n sigiro-fed logs -l app=sigiro-central --tail=50 || true
  exit 1
fi

kubectl apply -f "$REPO_ROOT/tests/k8s/federation/sigiro-collector-daemonset.yaml"
echo "Waiting for sigiro-collector pods..."
if ! kubectl -n sigiro-fed wait --for=condition=ready pod -l app=sigiro-collector --timeout=180s; then
  echo "ERROR: sigiro-collector pods did not become ready"
  kubectl -n sigiro-fed get pods -o wide
  kubectl -n sigiro-fed describe pod -l app=sigiro-collector
  kubectl -n sigiro-fed logs -l app=sigiro-collector --tail=50 || true
  exit 1
fi

# Verify k3s cluster has 3 nodes
echo "=== Verifying k3s cluster nodes ==="
for i in $(seq 1 30); do
  NODE_COUNT=$(kubectl get nodes -o json 2>/dev/null | python3 -c "import sys,json; print(len(json.load(sys.stdin)['items']))" || echo 0)
  if [[ "$NODE_COUNT" -ge 3 ]]; then
    echo "OK: $NODE_COUNT nodes in cluster"
    kubectl get nodes -o wide
    break
  fi
  echo "Waiting for collector nodes to join... ($i/30, current: $NODE_COUNT nodes)"
  sleep 5
done
if [[ "${NODE_COUNT:-0}" -lt 3 ]]; then
  echo "ERROR: Expected 3 k8s nodes, found ${NODE_COUNT:-0}"
  kubectl get nodes -o wide || true
  echo "--- collector-1 logs ---"
  vm_ssh "$FED_COLLECTOR_1" "sudo journalctl -u k3s-agent --no-pager -n 20" || true
  echo "--- collector-2 logs ---"
  vm_ssh "$FED_COLLECTOR_2" "sudo journalctl -u k3s-agent --no-pager -n 20" || true
  exit 1
fi

# ---------------------------------------------------------------------------
# Step 9 — wait for demo services
# ---------------------------------------------------------------------------
if [[ -f "$REPO_ROOT/tests/k8s/federation/demo-services.yaml" ]]; then
  echo "=== Deploying demo services ==="
  kubectl apply -f "$REPO_ROOT/tests/k8s/federation/demo-services.yaml"
  echo "Waiting for frontend to be ready..."
  if ! kubectl -n sigiro-fed wait --for=condition=ready deployment frontend --timeout=180s; then
    echo "WARNING: frontend deployment did not become ready"
    kubectl -n sigiro-fed get pods -o wide
  fi
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
    -d "{\"service\":\"frontend\",\"from\":$SINCE,\"to\":$NOW}")
echo "$INVESTIGATE_RESP" | jq . || echo "Raw: $INVESTIGATE_RESP"

# Run federated query
echo "--- /v1/query (federated) ---"
QUERY_RESP=$(kubectl exec -n sigiro-fed "$CENTRAL_POD" -- \
  curl -s -X POST "http://localhost:9999/v1/query" \
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
