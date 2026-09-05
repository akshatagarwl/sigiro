#!/bin/sh
set -eu
umask 077

if [ "${1:-}" = health ]; then
  status=$(supervisorctl -c /app/deploy/supervisord.conf status)
  printf '%s\n' "$status" | awk '$2 != "RUNNING" { exit 1 } END { if (NR != 3) exit 1 }'
  exec curl --fail --silent --output /dev/null --max-time 5 http://127.0.0.1:8080/.well-known/celld/health
fi

mkdir -p "$CLAUDE_CONFIG_DIR" "$GH_CONFIG_DIR" "$CODEX_HOME" \
  /var/lib/sigiro-agent/jobs /var/lib/sigiro-agent/receipts \
  /var/lib/sigiro-agent/evidence /var/lib/sigiro-agent/celld \
  /var/lib/sigiro-agent/eve/.workflow-data

# Native login commands share the running agent's persistent authentication paths.
if [ "$#" -gt 0 ]; then
  exec "$@"
fi

: "${S3_ENDPOINT:?S3_ENDPOINT is required}"
: "${CELLD_BUCKET:?CELLD_BUCKET is required}"
: "${AWS_ACCESS_KEY_ID:?AWS_ACCESS_KEY_ID is required}"
: "${AWS_SECRET_ACCESS_KEY:?AWS_SECRET_ACCESS_KEY is required}"
: "${SIGIRO_REPOSITORY:?SIGIRO_REPOSITORY is required}"
: "${EVE_SANDBOX_IMAGE:?EVE_SANDBOX_IMAGE is required}"

worker_token=/var/lib/sigiro-agent/worker.token
if [ ! -e "$worker_token" ]; then
  temporary_token=$(mktemp /var/lib/sigiro-agent/worker.token.XXXXXX)
  openssl rand -hex 32 > "$temporary_token"
  mv "$temporary_token" "$worker_token"
fi
EVE_WORKER_TOKEN=$(cat "$worker_token")
if [ "${#EVE_WORKER_TOKEN}" -ne 64 ]; then
  echo 'Stored worker token is invalid; restore the persistent token before starting.' >&2
  exit 1
fi
export EVE_WORKER_TOKEN
export CELLD_VAR_WORKER_TOKEN="$EVE_WORKER_TOKEN"
export AWS_REGION="${AWS_REGION:-us-east-1}"
export CELLD_WATCH=/var/lib/sigiro-agent/celld
export CELLD_DURABILITY=bucket
export CELLD_MAX_REQUEST_BODY_BYTES=1048576
export COORDINATOR_URL=http://127.0.0.1:8080
export EVE_URL=http://127.0.0.1:12000
export EXECUTOR_STATE_DIR=/var/lib/sigiro-agent/jobs
export CELLD_RECEIPTS_DIR=/var/lib/sigiro-agent/receipts
export SIGIRO_EVIDENCE_DIR=/var/lib/sigiro-agent/evidence
export WORKER_ID="${WORKER_ID:-sigiro-agent}"
export HOST=0.0.0.0
export PORT=12000

# Compose waits for MinIO health before starting this container. A failed bootstrap
# stops startup; supervision begins only after the production deployment succeeds.
mc --config-dir /tmp/sigiro-mc alias set local "$S3_ENDPOINT" "$AWS_ACCESS_KEY_ID" "$AWS_SECRET_ACCESS_KEY" >/dev/null
mc --config-dir /tmp/sigiro-mc mb --ignore-existing "local/$CELLD_BUCKET"
celld deploy /app/coordinator --bucket "$CELLD_BUCKET" --endpoint "$S3_ENDPOINT" --region "$AWS_REGION"

exec /usr/bin/supervisord -n -c /app/deploy/supervisord.conf
