#!/usr/bin/env bash
# Block until a dev-stack PostgreSQL container accepts connections.
#
# Alchemy's `Docker.Container` reconciles when the container is STARTED; the
# healthcheck it configures is observed by Docker, not awaited by the deploy.
# Without this the stack would report success while the first client still
# raced initdb — the exact class of failure the Alchemy migration exists to
# turn into a fast, local, obvious one.
set -euo pipefail

container="${SIGIRO_DEV_POSTGRES_CONTAINER:?SIGIRO_DEV_POSTGRES_CONTAINER is required}"
user="${SIGIRO_DEV_POSTGRES_USER:-sigiro}"
database="${SIGIRO_DEV_POSTGRES_DB:-}"

args=(-U "$user")
if [[ -n "$database" ]]; then
  args+=(-d "$database")
fi

for attempt in $(seq 1 60); do
  if docker exec "$container" pg_isready "${args[@]}" >/dev/null 2>&1; then
    echo "dev-wait-postgres: $container accepting connections"
    exit 0
  fi
  echo "dev-wait-postgres: $container not ready yet (attempt $attempt)" >&2
  sleep 1
done

echo "dev-wait-postgres: $container never became ready" >&2
docker logs --tail 50 "$container" >&2 || true
exit 1
