#!/usr/bin/env bash
# Run SDK validation: clone Pyroscope examples, start services, verify profiles.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

export DOCKER_BUILDKIT=1

# Cleanup on exit
cleanup() {
  echo "=== Cleaning up ==="
  docker compose down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Step 1: Clone Pyroscope examples ==="
bash setup.sh

echo "=== Step 2: Build images ==="
docker compose build

echo "=== Step 3: Start MinIO + Collector ==="
docker compose up -d minio minio-setup collector
echo "Waiting for collector to be ready..."
sleep 5

echo "=== Step 4: Start SDK apps + load generator ==="
docker compose up -d go-app python-app nodejs-app java-app load

echo "=== Step 5: Wait for profiles to accumulate ==="
# Node.js SDK has ~60s default push interval, need minimum 90s total wait.
# Go/Python push within 10-15s, Node.js takes ~60s for first push.
echo "Waiting 90s for all SDKs to push (Node.js SDK has ~60s push interval)..."
sleep 90

# Check flushes happened
FLUSH_COUNT=$(docker compose logs collector 2>&1 | grep -c "flushed parquet" || true)
echo "Found $FLUSH_COUNT flush(es) in collector logs"

if [ "$FLUSH_COUNT" -eq 0 ]; then
  echo "WARN: No flushes detected"
  echo "=== Collector logs ==="
  docker compose logs collector --tail=50
fi

# Extra wait for final flush cycle
echo "Waiting 20s for final flush cycle..."
sleep 20

echo "=== Step 6: Verify profiles ==="
bash verify.sh
