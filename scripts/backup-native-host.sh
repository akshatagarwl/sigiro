#!/usr/bin/env bash
set -euo pipefail

host=${SIGIRO_HOST:-root@91.98.118.235}
recipient=${SIGIRO_BACKUP_AGE_RECIPIENT:-age14vg795w9zy32m54aqcjegu98a5zgthue7zh0h2ywwu5rgtc9gqfqh3z8kr}
identity=${SIGIRO_BACKUP_AGE_IDENTITY:-$HOME/Library/Application Support/sops/age/keys.txt}
out=${1:-$HOME/.hermes/backups/host-systemd/$(date -u +%Y%m%dT%H%M%SZ)}
[[ -r "$identity" ]] || identity=$HOME/.config/sops/age/keys.txt
[[ -r "$identity" ]] || { printf 'age identity unavailable\n' >&2; exit 1; }
pg_restore_bin=${PG_RESTORE:-$(command -v pg_restore || true)}
[[ -n "$pg_restore_bin" ]] || pg_restore_bin=/opt/homebrew/opt/libpq/bin/pg_restore
[[ -x "$pg_restore_bin" ]] || { printf 'missing PostgreSQL 18 pg_restore\n' >&2; exit 1; }
[[ "$($pg_restore_bin --version)" == pg_restore\ \(PostgreSQL\)\ 18.* ]] || { printf 'PostgreSQL 18 pg_restore is required\n' >&2; exit 1; }
for command in age ssh tar shasum; do
  command -v "$command" >/dev/null || { printf 'missing command: %s\n' "$command" >&2; exit 1; }
done
mkdir -p "$out"
chmod 0700 "$out"
remote=(ssh -o BatchMode=yes "$host")

stream_encrypt() {
  local output=$1; shift
  "${remote[@]}" "$@" | age -r "$recipient" -o "$out/$output"
}

stream_encrypt linguabench-pipeline.pgdump.age \
  'docker exec -u postgres linguabench-postgres pg_dump -U postgres -d pipeline -Fc'
stream_encrypt linguabench-globals.sql.age \
  'docker exec -u postgres linguabench-postgres pg_dumpall -U postgres --globals-only'
stream_encrypt kamal-proxy-config.tar.age \
  'tar -C /var/lib/docker/volumes/kamal-proxy-config/_data --xattrs --acls --numeric-owner -cpf - .'

# The date and SQL execute on the remote host, not in this shell.
# shellcheck disable=SC2016
"${remote[@]}" 'set -e; printf "captured_at=%s\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"; docker exec -u postgres linguabench-postgres psql -U postgres -d pipeline -Atc "select current_setting('"'"'server_version'"'"'); select pg_database_size('"'"'pipeline'"'"'); select '"'"'responses'"'"',count(*) from responses union all select '"'"'response_judgments'"'"',count(*) from response_judgments union all select '"'"'judgment_poles'"'"',count(*) from judgment_poles order by 1;"' \
  > "$out/MANIFEST.txt"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
age -d -i "$identity" -o "$tmp/pipeline.dump" "$out/linguabench-pipeline.pgdump.age"
"$pg_restore_bin" --list "$tmp/pipeline.dump" >/dev/null
age -d -i "$identity" -o "$tmp/linguabench-globals.sql" "$out/linguabench-globals.sql.age"
[[ "$(wc -c < "$tmp/linguabench-globals.sql")" -gt 100 ]]
age -d -i "$identity" -o "$tmp/proxy.tar" "$out/kamal-proxy-config.tar.age"
tar -tf "$tmp/proxy.tar" >/dev/null
shasum -a 256 "$out"/*.age "$out/MANIFEST.txt" > "$out/SHA256SUMS"
(cd "$out" && shasum -a 256 -c SHA256SUMS)
printf '%s\n' "$out"
