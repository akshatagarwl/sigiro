#!/usr/bin/env bash
set -euo pipefail

mode=${1:-}
case "$mode" in
  rehearsal|cutover) ;;
  *) printf 'usage: %s rehearsal|cutover\n' "$0" >&2; exit 2 ;;
esac
[[ ${EUID:-$(id -u)} -eq 0 ]] || { printf 'must run as root\n' >&2; exit 1; }
command -v docker >/dev/null || { printf 'Docker is required only for the LinguaBench migration source\n' >&2; exit 1; }
command -v pg_restore >/dev/null || { printf 'PostgreSQL 18 client is required\n' >&2; exit 1; }
[[ "$(pg_restore --version)" == pg_restore\ \(PostgreSQL\)\ 18.* ]] || {
  printf 'PostgreSQL 18 pg_restore is required\n' >&2
  exit 1
}

pipeline_source=linguabench-postgres
# Sigiro's own catalog lives in a SEPARATE PostgreSQL container from
# LinguaBench's pipeline. Migrating only the pipeline is what silently emptied
# production: the Sigiro catalog holds the DuckLake metadata for every tenant,
# and without it the parquet telemetry is orphaned.
#
# Better Auth is NOT migrated. It runs as a Cloudflare Worker on D1 and its
# PostgreSQL database and role were retired from this host; restoring them here
# would resurrect a service that no longer exists.
catalog_source=linguabench-sigiro-catalog-db
target_port=${SIGIRO_POSTGRESQL_MIGRATION_PORT:-55432}
[[ "$(docker inspect -f '{{.State.Running}}' "$pipeline_source")" == true ]] || {
  printf 'source database is not running: %s\n' "$pipeline_source" >&2
  exit 1
}
[[ "$(docker inspect -f '{{.State.Running}}' "$catalog_source")" == true ]] || {
  printf 'Sigiro catalog source database is not running: %s\n' "$catalog_source" >&2
  exit 1
}
[[ -r /etc/sigiro/secrets/database.env ]] || {
  printf 'fresh Sigiro database credentials are unavailable\n' >&2
  exit 1
}
# shellcheck disable=SC1091
source /etc/sigiro/secrets/database.env
: "${SIGIRO_CATALOG_PASSWORD:?}"

if [[ "$mode" == cutover ]]; then
  running=$(docker ps --format '{{.Names}}' | grep -E '^(linguabench-(web|worker)-|linguabench-analysis$|linguabench-sigiro-hosted$)' || true)
  [[ -z "$running" ]] || { printf 'application writers must be stopped before final migration:\n%s\n' "$running" >&2; exit 1; }
fi

role_hash() {
  local container=$1 user=$2 role=$3 value
  value=$(docker exec -u postgres "$container" psql -U "$user" -d postgres -Atc "select rolpassword from pg_authid where rolname = '$role'")
  [[ "$value" =~ ^SCRAM-SHA-256\$4096:[A-Za-z0-9+/=]+\$[A-Za-z0-9+/=]+:[A-Za-z0-9+/=]+$ ]] || {
    printf 'source role does not have a valid SCRAM password: %s\n' "$role" >&2
    exit 1
  }
  printf '%s' "$value"
}

ensure_role() {
  local role=$1 password=$2
  sudo -u postgres psql -p "$target_port" --set=ON_ERROR_STOP=1 --set=password="$password" postgres >/dev/null <<SQL
DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$role') THEN
    CREATE ROLE $role LOGIN;
  END IF;
END
\$\$;
ALTER ROLE $role LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD :'password';
SQL
}

pipeline_hash=$(role_hash "$pipeline_source" postgres linguabench_app)
ensure_role linguabench_app "$pipeline_hash"
ensure_role sigiro "$SIGIRO_CATALOG_PASSWORD"
unset pipeline_hash SIGIRO_CATALOG_PASSWORD

source_metrics=$(mktemp)
target_metrics=$(mktemp)
trap 'rm -f "$source_metrics" "$target_metrics"' EXIT
docker exec -u postgres "$pipeline_source" psql -U postgres -d pipeline -Atc \
  "select 'pipeline.tables',count(*) from information_schema.tables where table_schema not in ('pg_catalog','information_schema'); select 'pipeline.responses',count(*) from responses; select 'pipeline.response_judgments',count(*) from response_judgments; select 'pipeline.judgment_poles',count(*) from judgment_poles;" \
  | sort > "$source_metrics"

# Record the Sigiro side too, so an empty restore fails the diff instead of
# being declared a success.
docker exec -u postgres "$catalog_source" psql -U sigiro -d sigiro -Atc \
  "select 'sigiro.tables',count(*) from information_schema.tables where table_schema not in ('pg_catalog','information_schema'); select 'sigiro.tenants',count(*) from sigiro_control.tenants; select 'sigiro.api_keys',count(*) from sigiro_control.api_keys;" \
  | sort >> "$source_metrics"

for database in pipeline sigiro; do
  sudo -u postgres dropdb -p "$target_port" --if-exists --force "$database"
done
sudo -u postgres createdb -p "$target_port" --owner=linguabench_app pipeline
sudo -u postgres createdb -p "$target_port" --owner=sigiro sigiro

docker exec -u postgres "$pipeline_source" pg_dump -U postgres -d pipeline --format=custom \
  | sudo -u postgres pg_restore -p "$target_port" --exit-on-error --dbname=pipeline

docker exec -u postgres "$catalog_source" pg_dump -U sigiro -d sigiro --format=custom \
  | sudo -u postgres pg_restore -p "$target_port" --exit-on-error --no-owner --role=sigiro --dbname=sigiro

sudo -u postgres psql -p "$target_port" --set=ON_ERROR_STOP=1 pipeline -c 'CREATE EXTENSION IF NOT EXISTS pg_cron' >/dev/null
sudo -u postgres psql -p "$target_port" --set=ON_ERROR_STOP=1 pipeline -c 'CREATE EXTENSION IF NOT EXISTS vector' >/dev/null
sudo -u postgres psql -p "$target_port" --set=ON_ERROR_STOP=1 sigiro -c 'CREATE EXTENSION IF NOT EXISTS vector' >/dev/null

sudo -u postgres psql -p "$target_port" -d pipeline -Atc \
  "select 'pipeline.tables',count(*) from information_schema.tables where table_schema not in ('pg_catalog','information_schema'); select 'pipeline.responses',count(*) from responses; select 'pipeline.response_judgments',count(*) from response_judgments; select 'pipeline.judgment_poles',count(*) from judgment_poles;" \
  | sort > "$target_metrics"
sudo -u postgres psql -p "$target_port" -d sigiro -Atc \
  "select 'sigiro.tables',count(*) from information_schema.tables where table_schema not in ('pg_catalog','information_schema'); select 'sigiro.tenants',count(*) from sigiro_control.tenants; select 'sigiro.api_keys',count(*) from sigiro_control.api_keys;" \
  | sort >> "$target_metrics"

diff -u "$source_metrics" "$target_metrics"
sudo -u postgres psql -p "$target_port" -Atc "select current_setting('server_version_num')::int >= 180000" | grep -qx t
sudo -u postgres psql -p "$target_port" -d pipeline -Atc "select extname from pg_extension where extname in ('pg_cron','vector') order by 1" | diff -u <(printf 'pg_cron\nvector\n') -
sudo -u postgres psql -p "$target_port" -d sigiro -Atc "select extname from pg_extension where extname = 'vector'" | grep -qx vector
# Sigiro must arrive POPULATED. The previous contract asserted these were empty,
# which is what let a cutover destroy the DuckLake catalog while still
# reporting success.
[[ "$(sudo -u postgres psql -p "$target_port" -d sigiro -Atc "select count(*) from sigiro_control.tenants")" -gt 0 ]] || {
  printf 'Sigiro catalog restored without tenants; DuckLake telemetry would be orphaned\n' >&2
  exit 1
}
[[ "$(sudo -u postgres psql -p "$target_port" -d sigiro -Atc "select count(*) from pg_namespace where nspname like 'tenant\_%'")" -gt 0 ]] || {
  printf 'Sigiro catalog restored without tenant DuckLake schemas\n' >&2
  exit 1
}
printf 'PostgreSQL 18 %s migration verified; LinguaBench and Sigiro both preserved\n' "$mode"
