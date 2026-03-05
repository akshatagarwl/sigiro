#!/usr/bin/env bash
# Deploy observe stack on OrbStack K8s and verify eBPF profiles with k8sattributes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K8S_CTX="orbstack"
NS="observe"

kubectl() { command kubectl --context "$K8S_CTX" "$@"; }

cleanup() {
  echo "=== Cleaning up ==="
  kubectl delete namespace "$NS" --ignore-not-found 2>/dev/null || true
  kubectl delete clusterrole observe-collector --ignore-not-found 2>/dev/null || true
  kubectl delete clusterrolebinding observe-collector --ignore-not-found 2>/dev/null || true
}

if [[ "${1:-}" == "--cleanup" ]]; then
  cleanup
  exit 0
fi

echo "=== Checking OrbStack K8s ==="
kubectl cluster-info || { echo "OrbStack K8s not running. Run: orbctl start k8s"; exit 1; }

echo ""
echo "=== Building collector image ==="
cd "$REPO_ROOT"
docker build -t observe-collector:local -f collector/Dockerfile .

echo ""
echo "=== Loading image into OrbStack K8s ==="
# OrbStack shares Docker images with K8s, no explicit load needed
# Verify the image exists
docker image inspect observe-collector:local > /dev/null

echo ""
echo "=== Deploying namespace + infrastructure ==="
kubectl apply -f "$SCRIPT_DIR/namespace.yaml"
kubectl apply -f "$SCRIPT_DIR/collector-rbac.yaml"
kubectl apply -f "$SCRIPT_DIR/minio.yaml"

echo "Waiting for MinIO..."
kubectl -n "$NS" rollout status deployment/minio --timeout=60s
kubectl -n "$NS" wait --for=condition=complete job/minio-setup --timeout=60s

echo ""
echo "=== Deploying collector DaemonSet ==="
kubectl apply -f "$SCRIPT_DIR/collector.yaml"
echo "Waiting for collector..."
kubectl -n "$NS" rollout status daemonset/observe-collector --timeout=120s

echo ""
echo "=== Deploying test workloads ==="
kubectl apply -f "$SCRIPT_DIR/workloads.yaml"
echo "Waiting for workloads..."
kubectl -n "$NS" rollout status deployment/cpu-burner --timeout=120s
kubectl -n "$NS" rollout status deployment/go-demo --timeout=120s
kubectl -n "$NS" rollout status deployment/traffic-gen --timeout=120s

echo ""
echo "=== Collecting telemetry (3 minutes) ==="
echo "eBPF profiler needs time to capture stacks..."
sleep 180

echo ""
echo "=== Checking collector logs ==="
COLLECTOR_POD=$(kubectl -n "$NS" get pods -l app=observe-collector -o jsonpath='{.items[0].metadata.name}')
echo "Collector pod: $COLLECTOR_POD"

echo ""
echo "--- Collector errors (if any) ---"
kubectl -n "$NS" logs "$COLLECTOR_POD" --tail=100 2>&1 | grep -iE "error|panic|fatal" | tail -20 || echo "(none)"

echo ""
echo "--- Profile-related logs ---"
kubectl -n "$NS" logs "$COLLECTOR_POD" --tail=500 2>&1 | grep -i "profil" | tail -20 || echo "(none)"

echo ""
echo "=== Verifying Parquet data via MinIO ==="
MINIO_POD=$(kubectl -n "$NS" get pods -l app=minio -o jsonpath='{.items[0].metadata.name}')

# Port-forward MinIO for DuckDB access
kubectl -n "$NS" port-forward "$MINIO_POD" 9000:9000 &
PF_PID=$!
sleep 3

if command -v duckdb &>/dev/null; then
  DUCKDB="duckdb"
else
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
  printf "  %-55s" "$name..."
  result=$($DUCKDB -noheader -csv -c "${S3_SETUP} ${query}" 2>&1) || true
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
echo "=== Profile checks ==="
run_check "Profiles have stacktrace" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0;" "true"
run_check "Profiles have value > 0" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND value > 0;" "true"
run_check "Profile services not all unknown" \
  "SELECT count(DISTINCT service) > 0 FROM $PARQUET WHERE signal='profile' AND service IS NOT NULL AND service <> '' AND service <> 'unknown_service';" "true"

echo ""
echo "=== K8s metadata checks (THE KEY TEST) ==="
run_check "Profiles have k8s.pod.name in metadata" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND metadata LIKE '%k8s.pod.name%';" "true"
run_check "Profiles have k8s.namespace.name in metadata" \
  "SELECT count(*) > 0 FROM $PARQUET WHERE signal='profile' AND metadata LIKE '%k8s.namespace.name%';" "true"

echo ""
echo "=== Data summary ==="
$DUCKDB -c "${S3_SETUP}
SELECT signal, count(*) as rows, count(DISTINCT service) as services
FROM $PARQUET GROUP BY signal ORDER BY signal;
" 2>&1 || true

echo ""
echo "=== Profile detail ==="
$DUCKDB -c "${S3_SETUP}
SELECT service, count(*) as samples,
       count(CASE WHEN stacktrace IS NOT NULL AND len(stacktrace) > 0 THEN 1 END) as with_stacks
FROM $PARQUET WHERE signal='profile'
GROUP BY service ORDER BY samples DESC LIMIT 10;
" 2>&1 || true

echo ""
echo "=== Top profile functions ==="
$DUCKDB -c "${S3_SETUP}
SELECT stacktrace[len(stacktrace)] AS leaf, count(*) AS samples
FROM $PARQUET WHERE signal='profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0
GROUP BY leaf ORDER BY samples DESC LIMIT 15;
" 2>&1 || true

echo ""
echo "=== Profile metadata sample ==="
$DUCKDB -c "${S3_SETUP}
SELECT service, metadata FROM $PARQUET WHERE signal='profile' AND metadata IS NOT NULL LIMIT 3;
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
