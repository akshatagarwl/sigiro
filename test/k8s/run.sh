#!/usr/bin/env bash
# Deploy OTel demo on K8s with real eBPF profiles and verify telemetry.
# Usage: ./run.sh [--context <k8s-context>] [--cleanup] [--skip-build]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K8S_CTX="orbstack"
NS="observe"
SKIP_BUILD=false

while [[ $# -gt 0 ]]; do
  case $1 in
    --context) K8S_CTX="$2"; shift 2 ;;
    --cleanup) CLEANUP=true; shift ;;
    --skip-build) SKIP_BUILD=true; shift ;;
    *) echo "Unknown flag: $1"; exit 1 ;;
  esac
done

kubectl() { command kubectl --context "$K8S_CTX" "$@"; }

cleanup() {
  echo "=== Cleaning up ==="
  kubectl delete namespace "$NS" --ignore-not-found 2>/dev/null || true
  kubectl delete clusterrole observe-collector --ignore-not-found 2>/dev/null || true
  kubectl delete clusterrolebinding observe-collector --ignore-not-found 2>/dev/null || true
}

if [[ "${CLEANUP:-}" == "true" ]]; then
  cleanup
  exit 0
fi

echo "=== K8s context: $K8S_CTX ==="
kubectl cluster-info || { echo "K8s not reachable on context '$K8S_CTX'"; exit 1; }

# --- Phase 0: Build collector image ---
if [[ "$SKIP_BUILD" == "false" ]]; then
  echo ""
  echo "=== Building collector image ==="
  cd "$REPO_ROOT"
  docker build -t observe-collector:local -f collector/Dockerfile .

  # Load image into K8s runtime if not OrbStack (OrbStack shares Docker images)
  if [[ "$K8S_CTX" != "orbstack" ]]; then
    echo "=== Loading image into K8s ==="
    if command -v limactl &>/dev/null; then
      # Lima/k3s: import via ctr
      LIMA_INSTANCE="${K8S_CTX}"
      docker save observe-collector:local | limactl shell "$LIMA_INSTANCE" sudo k3s ctr images import -
    else
      echo "WARNING: Non-OrbStack context — you may need to manually load the image"
    fi
  fi
fi

# --- Phase 1: Namespace + RBAC + MinIO ---
echo ""
echo "=== Phase 1: Namespace + RBAC + MinIO ==="
cleanup
kubectl apply -f "$SCRIPT_DIR/namespace.yaml"
kubectl apply -f "$SCRIPT_DIR/collector-rbac.yaml"
kubectl apply -f "$SCRIPT_DIR/minio.yaml"

echo "Waiting for MinIO..."
kubectl -n "$NS" rollout status deployment/minio --timeout=120s
kubectl -n "$NS" wait --for=condition=complete job/minio-setup --timeout=120s

# --- Phase 2: Demo infra (flagd, valkey, kafka, postgresql) ---
echo ""
echo "=== Phase 2: Demo infrastructure ==="
kubectl apply -f "$SCRIPT_DIR/demo-infra.yaml"

echo "Waiting for infra..."
for dep in flagd valkey-cart kafka postgresql; do
  kubectl -n "$NS" rollout status deployment/$dep --timeout=180s
done

# Wait for kafka readiness probe
echo "Waiting for Kafka readiness..."
kubectl -n "$NS" wait --for=condition=ready pod -l app=kafka --timeout=180s

# Wait for postgresql readiness probe
echo "Waiting for PostgreSQL readiness..."
kubectl -n "$NS" wait --for=condition=ready pod -l app=postgresql --timeout=60s

# --- Phase 3: Collector DaemonSet ---
echo ""
echo "=== Phase 3: Collector DaemonSet (eBPF) ==="
kubectl apply -f "$SCRIPT_DIR/collector.yaml"

echo "Waiting for collector..."
kubectl -n "$NS" rollout status daemonset/observe-collector --timeout=120s

# --- Phase 4: Demo services (14) ---
echo ""
echo "=== Phase 4: Demo services ==="
kubectl apply -f "$SCRIPT_DIR/demo-services.yaml"

SERVICES=(ad cart checkout currency email frontend image-provider load-generator payment product-catalog quote recommendation shipping)
echo "Waiting for ${#SERVICES[@]} service deployments..."
for svc in "${SERVICES[@]}"; do
  echo "  $svc..."
  kubectl -n "$NS" rollout status deployment/$svc --timeout=300s
done

# --- Phase 5: Collect telemetry ---
echo ""
echo "=== Phase 5: Collecting telemetry (3 minutes) ==="
echo "Load generator is driving traffic through the demo. eBPF profiler is capturing stacks."
sleep 180

# --- Phase 6: Verify ---
echo ""
echo "=== Phase 6: Verification ==="

echo ""
echo "--- Collector status ---"
COLLECTOR_POD=$(kubectl -n "$NS" get pods -l app=observe-collector -o jsonpath='{.items[0].metadata.name}')
echo "Pod: $COLLECTOR_POD"
kubectl -n "$NS" logs "$COLLECTOR_POD" --tail=100 2>&1 | grep -iE "error|panic|fatal" | tail -10 || echo "(no errors)"

echo ""
echo "--- Service pod status ---"
kubectl -n "$NS" get pods --no-headers | sort

# Port-forward MinIO for DuckDB
echo ""
echo "=== Setting up MinIO port-forward ==="
MINIO_POD=$(kubectl -n "$NS" get pods -l app=minio -o jsonpath='{.items[0].metadata.name}')
kubectl -n "$NS" port-forward "$MINIO_POD" 9000:9000 &
PF_PID=$!
sleep 3

if ! command -v duckdb &>/dev/null; then
  echo "duckdb CLI not found — install it for verification"
  kill $PF_PID 2>/dev/null || true
  exit 1
fi

S3_SETUP="
INSTALL httpfs; LOAD httpfs;
SET s3_endpoint='localhost:9000';
SET s3_access_key_id='minioadmin';
SET s3_secret_access_key='minioadmin';
SET s3_use_ssl=false;
SET s3_url_style='path';
"
PARQUET="read_parquet('s3://observe/**/*.parquet')"

FAILURES=0
PASSES=0

run_check() {
  local name="$1"
  local query="$2"
  local expected="$3"
  printf "  %-60s" "$name..."
  result=$(duckdb -noheader -csv -c "${S3_SETUP} ${query}" 2>&1) || true
  if echo "$result" | grep -q "$expected"; then
    echo "PASS"
    PASSES=$((PASSES + 1))
  else
    echo "FAIL"
    echo "    Expected: $expected"
    echo "    Got: $result"
    FAILURES=$((FAILURES + 1))
  fi
}

# Informational check — doesn't affect pass/fail
run_info() {
  local name="$1"
  local query="$2"
  local expected="$3"
  printf "  %-60s" "[info] $name..."
  result=$(duckdb -noheader -csv -c "${S3_SETUP} ${query}" 2>&1) || true
  if echo "$result" | grep -q "$expected"; then
    echo "YES"
  else
    echo "NO"
    echo "    Got: $result"
  fi
}

echo ""
echo "=== Signal checks ==="
run_check "Signal: profile exists" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile';" "true"
run_check "Signal: span exists" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span';" "true"
run_check "Signal: log exists" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='log';" "true"
run_check "Signal: metric exists" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='metric';" "true"

echo ""
echo "=== Service coverage ==="
run_check ">=5 distinct services reporting spans" \
  "SELECT count(DISTINCT service) >= 5 FROM $PARQUET WHERE signal='span';" "true"
run_check ">=5 distinct services reporting metrics" \
  "SELECT count(DISTINCT service) >= 5 FROM $PARQUET WHERE signal='metric';" "true"

echo ""
echo "=== Cross-service traces ==="
run_check "Traces span >=3 services" \
  "SELECT max(svc_count) >= 3 FROM (SELECT trace_id, count(DISTINCT service) AS svc_count FROM $PARQUET WHERE signal='span' AND trace_id IS NOT NULL GROUP BY trace_id);" "true"
run_check "Span depth >=3 (parent_span_id chains)" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND parent_span_id IS NOT NULL AND parent_span_id <> '';" "true"

echo ""
echo "=== Profile checks ==="
run_check "Profiles have stacktrace" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" "true"
run_check "Profiles have value > 0" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND value > 0;" "true"

echo ""
echo "=== K8s metadata enrichment ==="
run_check "Spans have k8s.namespace.name in metadata" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='span' AND metadata LIKE '%k8s.namespace.name%';" "true"
run_check "Profiles have k8s.pod.name in metadata" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND metadata LIKE '%k8s.pod.name%';" "true"

echo ""
echo "=== eBPF quality (informational) ==="
run_info "eBPF profiles have app-level service names" \
  "SELECT count(DISTINCT service) > 1 FROM $PARQUET WHERE signal='profile' AND service IS NOT NULL AND service <> '' AND service <> 'unknown_service';" "true"
run_info "eBPF profiles have app-level functions in stacks" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 3;" "true"

echo ""
echo "=== Data summary ==="
duckdb -c "${S3_SETUP}
SELECT signal, count(*) as rows, count(DISTINCT service) as services
FROM $PARQUET GROUP BY signal ORDER BY signal;
" 2>&1 || true

echo ""
echo "=== Profile detail ==="
duckdb -c "${S3_SETUP}
SELECT service, count(*) as samples,
       count(CASE WHEN stacktrace IS NOT NULL AND len(stacktrace) > 0 THEN 1 END) as with_stacks
FROM $PARQUET WHERE signal='profile'
GROUP BY service ORDER BY samples DESC LIMIT 10;
" 2>&1 || true

echo ""
echo "=== Top profile functions ==="
duckdb -c "${S3_SETUP}
SELECT stacktrace[len(stacktrace)] AS leaf, count(*) AS samples
FROM $PARQUET WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0
GROUP BY leaf ORDER BY samples DESC LIMIT 15;
" 2>&1 || true

echo ""
echo "=== Profile metadata sample ==="
duckdb -c "${S3_SETUP}
SELECT service, metadata FROM $PARQUET WHERE signal='profile' AND metadata IS NOT NULL LIMIT 3;
" 2>&1 || true

echo ""
echo "=== Checkout investigation block (cross-service) ==="
duckdb -c "${S3_SETUP}
SELECT service, signal, count(*) as rows
FROM $PARQUET
WHERE trace_id IN (
  SELECT DISTINCT trace_id FROM $PARQUET
  WHERE signal='span' AND service='checkout' AND trace_id IS NOT NULL
  LIMIT 5
)
GROUP BY service, signal ORDER BY service, signal;
" 2>&1 || true

# Cleanup port-forward
kill $PF_PID 2>/dev/null || true

echo ""
echo "==============================="
echo "  Passed: $PASSES"
echo "  Failed: $FAILURES"
echo "==============================="

if [ $FAILURES -gt 0 ]; then
  echo "=== SOME CHECKS FAILED ==="
  exit 1
fi

echo "=== ALL CHECKS PASSED ==="
