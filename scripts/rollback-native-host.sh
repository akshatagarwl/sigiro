#!/usr/bin/env bash
set -euo pipefail

[[ ${1:-} == --accept-native-write-loss ]] || {
  printf 'usage: %s --accept-native-write-loss\n' "$0" >&2
  printf 'rollback restores the retained Docker databases and discards writes accepted only by native PostgreSQL\n' >&2
  exit 2
}
[[ ${EUID:-$(id -u)} -eq 0 ]] || { printf 'must run as root\n' >&2; exit 1; }
receipt=/var/lib/sigiro-migration/docker-rollback.env
[[ -r "$receipt" ]] || { printf 'Docker rollback receipt is missing\n' >&2; exit 1; }
# shellcheck disable=SC1090
source "$receipt"
: "${LINGUABENCH_WEB:?missing rollback web container}"
: "${LINGUABENCH_WORKER:?missing rollback worker container}"

systemctl stop kamal-proxy.service sigiro.service sigiro-edge.service \
  linguabench-web.service linguabench-worker.service linguabench-analysis.timer linguabench-analysis.service \
  postgresql@18-main.service || true
rm -f /var/lib/sigiro-migration/native-active

containers=(
  linguabench-postgres
  linguabench-sigiro-catalog-db
  linguabench-sigiro-edge
  linguabench-sigiro-hosted
  "$LINGUABENCH_WEB"
  "$LINGUABENCH_WORKER"
  linguabench-analysis
  linguabench-registry
  kamal-proxy
)
for container in "${containers[@]}"; do
  docker update --restart=unless-stopped "$container" >/dev/null
  docker start "$container" >/dev/null
done

for container in linguabench-postgres linguabench-sigiro-catalog-db linguabench-sigiro-hosted; do
  for _ in $(seq 1 60); do
    status=$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container")
    [[ "$status" == healthy || "$status" == running ]] && break
    sleep 2
  done
  status=$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container")
  [[ "$status" == healthy || "$status" == running ]] || { printf 'rollback container unhealthy: %s (%s)\n' "$container" "$status" >&2; exit 1; }
done

curl --fail --silent --show-error --retry 30 --retry-delay 2 --retry-all-errors https://sigiro.com/health >/dev/null
curl --fail --silent --show-error --retry 30 --retry-delay 2 --retry-all-errors https://linguabench.akshat.dev/up >/dev/null
printf 'Docker rollback restored; native PostgreSQL remains stopped for forensic recovery\n'
