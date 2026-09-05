#!/bin/sh
set -eu
umask 077

if [ "${1:-}" = health ]; then
  status=$(supervisorctl -c /app/deploy/supervisord.conf status)
  printf '%s\n' "$status" | awk '$2 != "RUNNING" { exit 1 } END { if (NR != 2) exit 1 }'
  world_secret=$(tr -d '\n' < /var/lib/sigiro-agent/world.secret)
  [ "${#world_secret}" -eq 64 ]
  curl --fail --silent --output /dev/null --max-time 5 \
    -H "Authorization: Bearer $world_secret" \
    http://127.0.0.1:8080/v1/health
  exec curl --fail --silent --output /dev/null --max-time 5 http://127.0.0.1:12000/eve/v1/health
fi

mkdir -p "$CLAUDE_CONFIG_DIR" "$GH_CONFIG_DIR" "$CODEX_HOME" \
  /var/lib/sigiro-agent/jobs /var/lib/sigiro-agent/receipts \
  /var/lib/sigiro-agent/evidence /var/lib/sigiro-agent/celld \
  /var/lib/sigiro-agent/eve/.workflow-data

if [ "$#" -gt 0 ]; then
  exec "$@"
fi

: "${CELLD_BUCKET:?CELLD_BUCKET is required}"
: "${AWS_ACCESS_KEY_ID:?AWS_ACCESS_KEY_ID is required}"
: "${AWS_SECRET_ACCESS_KEY:?AWS_SECRET_ACCESS_KEY is required}"
: "${SIGIRO_REPOSITORY:?SIGIRO_REPOSITORY is required}"
: "${BOX_API_KEY:?BOX_API_KEY is required}"

secret_file=/var/lib/sigiro-agent/world.secret
callback_secret_file=/var/lib/sigiro-agent/workflow-callback.secret
for file in "$secret_file" "$callback_secret_file"; do
  if [ ! -e "$file" ]; then
    temporary_file=$(mktemp "$file.XXXXXX")
    openssl rand -hex 32 > "$temporary_file"
    mv "$temporary_file" "$file"
  fi
  [ "$(wc -c < "$file")" -eq 65 ] || {
    echo "Stored secret is invalid; restore the persistent secret before starting." >&2
    exit 1
  }
done

CELLD_WORLD_SECRET=$(tr -d '\n' < "$secret_file")
WORKFLOW_CALLBACK_SECRET=$(tr -d '\n' < "$callback_secret_file")
export CELLD_WORLD_SECRET WORKFLOW_CALLBACK_SECRET
export CELLD_VAR_WORLD_SECRET="$CELLD_WORLD_SECRET"
export CELLD_VAR_WORKFLOW_CALLBACK_SECRET="$WORKFLOW_CALLBACK_SECRET"
export CELLD_VAR_WORKFLOW_RETENTION_MS=7776000000
export CELLD_FLEET_URL=http://127.0.0.1:8080
export WORKFLOW_BASE_URL=http://127.0.0.1:12000
export AWS_REGION="${AWS_REGION:-us-east-1}"
export CELLD_WATCH=/var/lib/sigiro-agent/celld
export CELLD_DURABILITY=bucket
export CELLD_MAX_REQUEST_BODY_BYTES=1048576
export EXECUTOR_STATE_DIR=/var/lib/sigiro-agent/jobs
export CELLD_RECEIPTS_DIR=/var/lib/sigiro-agent/receipts
export SIGIRO_EVIDENCE_DIR=/var/lib/sigiro-agent/evidence
export HOST=127.0.0.1
export PORT=12000

celld deploy /app/world-celld-worker --bucket "$CELLD_BUCKET" --region "$AWS_REGION"

exec /usr/bin/supervisord -n -c /app/deploy/supervisord.conf
