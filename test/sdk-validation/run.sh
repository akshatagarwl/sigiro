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
TIMEOUT=120
ELAPSED=0
while [ $ELAPSED -lt $TIMEOUT ]; do
  FLUSH_COUNT=$(docker compose logs collector 2>&1 | grep -c "flushed parquet" || true)
  if [ "$FLUSH_COUNT" -ge 3 ]; then
    echo "Found $FLUSH_COUNT flush(es) in collector logs after ${ELAPSED}s"
    break
  fi
  sleep 5
  ELAPSED=$((ELAPSED + 5))
done

if [ $ELAPSED -ge $TIMEOUT ]; then
  echo "WARN: Only $FLUSH_COUNT flush(es) after ${TIMEOUT}s, proceeding anyway..."
  echo "=== Collector logs ==="
  docker compose logs collector --tail=50
fi

# Extra wait for final flush cycle
echo "Waiting 30s for final flush cycle..."
sleep 30

echo "=== Step 6: Verify profiles ==="
bash verify.sh
