#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
host=${SIGIRO_HOST:-root@91.98.118.235}
ssh_key=${SIGIRO_SSH_KEY:-$HOME/.ssh/linguabench}
mode=${SIGIRO_NATIVE_ACTION:-check}
while (($#)); do
  case "$1" in
    --check|--deploy|--verify) mode=${1#--}; shift ;;
    --host) host=${2:?}; shift 2 ;;
    --ssh-key) ssh_key=${2:?}; shift 2 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

required=(
  ops/prod/native-host.sh
  ops/prod/configure-native-routes.sh
  ops/prod/postgresql.conf
  ops/prod/pg_hba.conf
  ops/prod/sigiro-edge.nginx.conf
  ops/prod/environment/sigiro.env
  ops/prod/environment/linguabench-profiler.env
  ops/prod/systemd/linguabench-profiler.conf
  ops/optimizer/read-bounded-body.mjs
  ops/optimizer/relay-target.mjs
  scripts/install-verified-artifact.py
  scripts/render-native-secrets.py
)
for path in "${required[@]}"; do
  [[ -f "$repo/$path" ]] || { printf 'native contract is missing %s\n' "$path" >&2; exit 1; }
done
for unit in sigiro sigiro-edge kamal-proxy kamal-proxy-routes; do
  [[ -f "$repo/ops/prod/systemd/$unit.service" ]] || { printf 'missing unit: %s\n' "$unit" >&2; exit 1; }
done
[[ "$mode" == check ]] && { printf 'native contract valid\n'; exit 0; }
[[ -r "$ssh_key" ]] || { printf 'SSH key unavailable: %s\n' "$ssh_key" >&2; exit 1; }
ssh_opts=(-i "$ssh_key" -o BatchMode=yes -o StrictHostKeyChecking=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=8)
remote=(ssh "${ssh_opts[@]}" "$host")

if [[ "$mode" == verify ]]; then
  "${remote[@]}" "/usr/local/sbin/sigiro-native-host verify"
  exit 0
fi
[[ "$mode" == deploy ]] || { printf 'unknown mode: %s\n' "$mode" >&2; exit 2; }

artifacts=(
  "sigiro|${SIGIRO_ARTIFACT:-}|${SIGIRO_ARTIFACT_SHA256:-}"
  "node|${NODE_ARTIFACT:-}|${NODE_ARTIFACT_SHA256:-}"
  "kamal-proxy|${KAMAL_PROXY_ARTIFACT:-}|${KAMAL_PROXY_ARTIFACT_SHA256:-}"
  "linguabench|${LINGUABENCH_ARTIFACT:-}|${LINGUABENCH_ARTIFACT_SHA256:-}"
)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/contract/systemd" "$tmp/contract/environment" "$tmp/contract/artifacts" "$tmp/artifacts"
cp "$repo/ops/prod/systemd/"*.service "$tmp/contract/systemd/"
cp "$repo/ops/prod/systemd/linguabench-ingest-relay.conf" "$tmp/contract/systemd/"
cp "$repo/ops/prod/systemd/linguabench-profiler.conf" "$tmp/contract/systemd/"
cp "$repo/ops/optimizer/ingest-relay.mjs" "$tmp/contract/ingest-relay.mjs"
cp "$repo/ops/optimizer/read-bounded-body.mjs" "$tmp/contract/read-bounded-body.mjs"
cp "$repo/ops/optimizer/relay-target.mjs" "$tmp/contract/relay-target.mjs"
if compgen -G "$repo/ops/prod/systemd/*.timer" >/dev/null; then
  cp "$repo/ops/prod/systemd/"*.timer "$tmp/contract/systemd/"
fi
cp "$repo/ops/prod/environment/"*.env "$tmp/contract/environment/"
cp "$repo/ops/prod/native-host.sh" "$tmp/contract/native-host.sh"
cp "$repo/ops/prod/configure-native-routes.sh" "$tmp/contract/configure-native-routes.sh"
cp "$repo/ops/prod/postgresql.conf" "$tmp/contract/postgresql.conf"
cp "$repo/ops/prod/pg_hba.conf" "$tmp/contract/pg_hba.conf"
cp "$repo/ops/prod/sigiro-edge.nginx.conf" "$tmp/contract/sigiro-edge.nginx.conf"
cp "$repo/scripts/install-verified-artifact.py" "$tmp/contract/install-verified-artifact.py"

: > "$tmp/contract/artifacts/manifest"
for entry in "${artifacts[@]}"; do
  IFS='|' read -r service archive digest <<< "$entry"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || { printf 'invalid %s artifact digest\n' "$service" >&2; exit 1; }
  filename="$service.tar.gz"
  if [[ "$archive" == installed ]]; then
    [[ "$service" != sigiro ]] || { printf 'Sigiro requires a freshly verified release artifact\n' >&2; exit 1; }
    "${remote[@]}" "test \"\$(readlink -f /opt/$service/current)\" = /opt/$service/releases/$digest" < /dev/null || {
      printf 'already-installed %s artifact does not match its pinned digest\n' "$service" >&2; exit 1;
    }
  else
    [[ -f "$archive" ]] || { printf 'missing %s artifact\n' "$service" >&2; exit 1; }
    actual=$(shasum -a 256 "$archive" | cut -d' ' -f1)
    [[ "$actual" == "$digest" ]] || { printf 'checksum mismatch for %s\n' "$service" >&2; exit 1; }
    cp "$archive" "$tmp/artifacts/$filename"
  fi
  printf '%s|%s|%s\n' "$service" "$filename" "$digest" >> "$tmp/contract/artifacts/manifest"
done

python3 "$repo/scripts/render-native-secrets.py" --output "$tmp/contract/secrets"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C "$tmp/contract" -czf "$tmp/contract.tar.gz" . 2>/dev/null \
  || COPYFILE_DISABLE=1 tar -C "$tmp/contract" -czf "$tmp/contract.tar.gz" .
! tar -tzf "$tmp/contract.tar.gz" | grep -q '/\._' || { printf 'contract archive contains AppleDouble metadata\n' >&2; exit 1; }
stage=/var/cache/sigiro-native-stage
# Artifacts (~500 MB) move by rsync, not a single SSH stdin pipe: one stalled pipe
# kills the whole transfer with no resume, and a re-run restarts from zero.
# --partial keeps a half-sent archive so a retry continues where it stopped.
"${remote[@]}" "set -euo pipefail; install -d -m 0700 $stage $stage/artifacts" < /dev/null
rsync -rlptz --partial --timeout=120 \
  -e "ssh ${ssh_opts[*]}" \
  "$tmp/artifacts/" "$host:$stage/artifacts/"
"${remote[@]}" "set -euo pipefail; tar -xzf - -C $stage; install -m 0755 $stage/native-host.sh /usr/local/sbin/sigiro-native-host; /usr/local/sbin/sigiro-native-host $mode; rm -f /usr/local/sbin/rollback-native-host /usr/local/sbin/migrate-postgresql18 /usr/local/sbin/render-linguabench-environment; rm -rf $stage/artifacts" < "$tmp/contract.tar.gz"
