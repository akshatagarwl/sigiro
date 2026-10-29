#!/usr/bin/env bash
# Run test/worker-agent-e2e.ts against a local auth Worker on a fresh local D1.
# The preview env supplies AUTH_TEST_OTP; the local origin and a throwaway
# secret replace its deployed URLs and its `wrangler secret`.
set -euo pipefail
cd "$(dirname "$0")/.."
port=9876
base="http://127.0.0.1:$port"
state=$(mktemp -d)
log="$state/wrangler-dev.log"
dev_pid=""
cleanup() {
  if [[ -n "$dev_pid" ]]; then
    kill "$dev_pid" 2>/dev/null || true
    wait "$dev_pid" 2>/dev/null || true
  fi
  rm -rf "$state"
}
trap cleanup EXIT

aube run --silent schema:sql > "$state/schema.sql"
wrangler d1 execute DB --local --env preview --persist-to "$state/d1" --file "$state/schema.sql" >/dev/null

wrangler dev --env preview --port "$port" --persist-to "$state/d1" \
  --var "BETTER_AUTH_URL:$base" --var "SIGIRO_RESOURCE:$base" \
  --var "BETTER_AUTH_SECRET:local-e2e-secret-at-least-32-characters-long" \
  > "$log" 2>&1 &
dev_pid=$!

for _ in $(seq 1 60); do
  if curl -fsS -o /dev/null "$base/api/auth/ok" 2>/dev/null; then
    SIGIRO_E2E_BASE="$base" node --import tsx test/worker-agent-e2e.ts
    exit
  fi
  if ! kill -0 "$dev_pid" 2>/dev/null; then
    cat "$log" >&2
    echo "agent-e2e: wrangler dev exited before serving $base" >&2
    exit 1
  fi
  sleep 1
done
cat "$log" >&2
echo "agent-e2e: $base never answered /api/auth/ok" >&2
exit 1
