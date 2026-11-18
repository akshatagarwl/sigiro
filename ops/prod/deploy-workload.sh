#!/usr/bin/env bash
set -euo pipefail

context="${SIGIRO_DOCKER_CONTEXT:-sigiro-prod}"
network="${SIGIRO_DOCKER_NETWORK:-sigiro}"
compose_file="${SIGIRO_COMPOSE_FILE:-ops/prod/compose.yaml}"
project="sigiro"

required=(
  SIGIRO_IMAGE
  SIGIRO_ROLLBACK_IMAGE
  SIGIRO_CATALOG_PASSWORD
  SIGIRO_AUTH_DB_TLS_CERT_B64
  SIGIRO_AUTH_DB_TLS_KEY_B64
  SIGIRO_ADMIN_TOKEN
)
for name in "${required[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    printf 'deploy-workload: required environment variable %s is unavailable\n' "$name" >&2
    exit 1
  fi
done

validate_image() {
  case "$1" in
    ghcr.io/sigiroai/sigiro@sha256:????????????????????????????????????????????????????????????????) ;;
    *)
      printf 'deploy-workload: image must be an immutable ghcr.io/sigiroai/sigiro digest\n' >&2
      return 1
      ;;
  esac
}
validate_image "$SIGIRO_IMAGE"
validate_image "$SIGIRO_ROLLBACK_IMAGE"

for resource in \
  "network $network" \
  'volume linguabench_sigiro_catalog_data' \
  'volume linguabench_sigiro_hosted_data'; do
  read -r kind name <<<"$resource"
  docker --context "$context" "$kind" inspect "$name" >/dev/null
done

if ! docker --context "$context" container inspect kamal-proxy >/dev/null 2>&1; then
  printf 'deploy-workload: shared host ingress container is unavailable\n' >&2
  exit 1
fi
proxy_networks="$(docker --context "$context" container inspect --format '{{json .NetworkSettings.Networks}}' kamal-proxy)"
if [[ "$proxy_networks" != *'"sigiro"'* ]]; then
  docker --context "$context" network connect "$network" kamal-proxy
fi

containers=(
  linguabench-sigiro-catalog-db
  linguabench-sigiro-edge
  linguabench-sigiro-hosted
)
for name in "${containers[@]}"; do
  if ! docker --context "$context" container inspect "$name" >/dev/null 2>&1; then
    continue
  fi
  owner="$(docker --context "$context" container inspect --format '{{ index .Config.Labels "com.docker.compose.project" }}' "$name")"
  if [[ "$owner" != "$project" ]]; then
    printf 'deploy-workload: %s exists but is not owned by Docker Compose project %s; explicit one-time cutover is required\n' "$name" "$project" >&2
    exit 1
  fi
done

compose=(docker --context "$context" compose --project-name "$project" --file "$compose_file")
proxy=(docker --context "$context" exec kamal-proxy kamal-proxy deploy)

container_id() {
  local id
  id="$(docker --context "$context" container inspect --format '{{.Id}}' "$1")"
  printf '%s\n' "${id:0:12}"
}

update_bootstrap_routes() {
  "${proxy[@]}" linguabench-sigiro-edge \
    --target "$(container_id linguabench-sigiro-edge):8080" \
    --host api-origin.sigiro.com \
    --tls \
    --target-timeout 5s \
    --buffer-requests \
    --buffer-responses \
    --buffer-memory 65536 \
    --max-request-body 65536 \
    --max-response-body 1048576 \
    --health-check-path /health \
    --health-check-interval 3s \
    --health-check-timeout 3s
}

update_hosted_route() {
  "${proxy[@]}" linguabench-sigiro-hosted \
    --target "$(container_id linguabench-sigiro-hosted):9999" \
    --host api-origin.sigiro.com \
    --forward-headers=false \
    --path-prefix /v1 \
    --path-prefix /v1development/profiles \
    --path-prefix /health \
    --path-prefix /healthz \
    --path-prefix /ingest \
    --path-prefix /push.v1.PusherService/Push \
    --path-prefix /openapi.json \
    --path-prefix /.well-known/oauth-protected-resource \
    --strip-path-prefix=false \
    --target-timeout 90s \
    --buffer-requests \
    --buffer-responses \
    --buffer-memory 1048576 \
    --max-request-body 8388608 \
    --max-response-body 16777216 \
    --health-check-path /health \
    --health-check-interval 3s \
    --health-check-timeout 5s || return
}

deploy_image() {
  local image="$1"
  if ! SIGIRO_IMAGE="$image" "${compose[@]}" config --quiet; then
    return 1
  fi
  if ! SIGIRO_IMAGE="$image" "${compose[@]}" up --detach --wait --wait-timeout 180 catalog-db edge; then
    return 1
  fi
  if ! update_bootstrap_routes; then
    return 1
  fi
  if ! SIGIRO_IMAGE="$image" "${compose[@]}" up --detach --wait --wait-timeout 180 hosted; then
    return 1
  fi
  if ! update_hosted_route; then
    return 1
  fi
  live_image="$(docker --context "$context" container inspect --format '{{.Config.Image}}' linguabench-sigiro-hosted)"
  [[ "$live_image" == "$image" ]]
}

if deploy_image "$SIGIRO_IMAGE"; then
  printf 'deployed %s through Alchemy-managed Docker Compose\n' "$SIGIRO_IMAGE"
  exit 0
fi

printf 'deploy-workload: deployment failed; restoring %s\n' "$SIGIRO_ROLLBACK_IMAGE" >&2
if ! deploy_image "$SIGIRO_ROLLBACK_IMAGE"; then
  printf 'deploy-workload: rollback failed\n' >&2
  exit 1
fi
printf 'deploy-workload: rollback completed\n' >&2
exit 1
