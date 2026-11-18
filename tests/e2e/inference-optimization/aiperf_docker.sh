#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
IMAGE=sigiro-aiperf-e2e:0.12.0
CACHE="$ROOT/target/inference-optimization-e2e/huggingface-cache"
mkdir -p "$CACHE"

# Sigiro runs on the host during this Mac/Linux-container E2E. Rewrite only its
# OTLP endpoint; the manifest already names host.docker.internal for inference.
args=()
rewrite_next=false
for arg in "$@"; do
  if $rewrite_next; then
    args+=("${arg/127.0.0.1/host.docker.internal}")
    rewrite_next=false
  else
    args+=("$arg")
    [[ "$arg" == "--otel-url" ]] && rewrite_next=true
  fi
done

name="sigiro-aiperf-e2e-$$"
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 143' HUP INT TERM

docker run --rm --init --name "$name" \
  --add-host host.docker.internal:host-gateway \
  -v "$ROOT:$ROOT" \
  -v "$CACHE:/root/.cache/huggingface" \
  -w "$PWD" \
  "$IMAGE" "${args[@]}"
