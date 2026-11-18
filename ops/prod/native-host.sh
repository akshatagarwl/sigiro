#!/usr/bin/env bash
set -euo pipefail

stage=/var/cache/sigiro-native-stage
command=${1:-}
[[ ${EUID:-$(id -u)} -eq 0 ]] || { printf 'must run as root\n' >&2; exit 1; }

require_stage() {
  [[ -d "$stage" ]] || { printf 'native stage is missing: %s\n' "$stage" >&2; exit 1; }
}

install_contract() {
  install -d -m 0755 /etc/sigiro /etc/linguabench /usr/local/libexec /etc/systemd/system/postgresql@18-main.service.d
  install -d -m 0700 /etc/sigiro/secrets
  install -m 0644 "$stage"/systemd/*.service /etc/systemd/system/
  install -m 0755 "$stage/ingest-relay.mjs" /usr/local/libexec/sigiro-ingest-relay.mjs
  install -m 0644 "$stage/read-bounded-body.mjs" /usr/local/libexec/read-bounded-body.mjs
  install -m 0644 "$stage/relay-target.mjs" /usr/local/libexec/relay-target.mjs
  for unit in linguabench-web linguabench-worker linguabench-analysis; do
    install -d -m 0755 "/etc/systemd/system/$unit.service.d"
    install -m 0644 "$stage/systemd/linguabench-ingest-relay.conf" "/etc/systemd/system/$unit.service.d/ingest-relay.conf"
    if [[ "$unit" != linguabench-analysis ]]; then
      install -m 0644 "$stage/systemd/linguabench-profiler.conf" "/etc/systemd/system/$unit.service.d/profiler.conf"
    fi
  done
  install -m 0644 "$stage/environment/linguabench-profiler.env" /etc/linguabench/profiler.env
  if compgen -G "$stage/systemd/*.timer" >/dev/null; then
    install -m 0644 "$stage"/systemd/*.timer /etc/systemd/system/
  fi
  install -m 0644 "$stage/environment/sigiro.env" /etc/sigiro/environment
  install -m 0644 "$stage/sigiro-edge.nginx.conf" /etc/sigiro/nginx-edge.conf
  install -m 0755 "$stage/configure-native-routes.sh" /usr/local/sbin/configure-native-routes
  install -m 0755 "$stage/install-verified-artifact.py" /usr/local/sbin/install-verified-artifact
  install -m 0644 "$stage/postgresql.conf" /etc/postgresql/18/main/sigiro.conf
  install -m 0640 -o postgres -g postgres "$stage/pg_hba.conf" /etc/postgresql/18/main/pg_hba.conf
  grep -qxF "include_if_exists = '/etc/postgresql/18/main/sigiro.conf'" /etc/postgresql/18/main/postgresql.conf \
    || printf "\ninclude_if_exists = '/etc/postgresql/18/main/sigiro.conf'\n" >> /etc/postgresql/18/main/postgresql.conf
  install -m 0644 "$stage/secrets/server.crt" /etc/postgresql/18/main/server.crt
  install -m 0600 -o postgres -g postgres "$stage/secrets/server.key" /etc/postgresql/18/main/server.key
  install -m 0644 "$stage/secrets/server.crt" /etc/sigiro/postgres-ca.crt
  install -m 0644 "$stage/secrets/server.crt" /etc/linguabench/postgres-ca.crt
  install -m 0600 "$stage/secrets/sigiro.env" /etc/sigiro/secrets/sigiro.env
  install -m 0600 "$stage/secrets/database.env" /etc/sigiro/secrets/database.env
  install -m 0600 "$stage/secrets/ingest-relay.env" /etc/sigiro/secrets/ingest-relay.env
  install -m 0600 "$stage/secrets/linguabench-relay.env" /etc/sigiro/secrets/linguabench-relay.env
  grep -qE '^[[:space:]]*127\.0\.0\.1[[:space:]]+auth-db\.sigiro\.com([[:space:]]|$)' /etc/hosts \
    || printf '127.0.0.1 auth-db.sigiro.com\n' >> /etc/hosts
}

install_artifacts() {
  local service archive digest
  local required=3145728 available
  available=$(df --output=avail -k /opt | tail -n1)
  [[ "$available" -ge "$required" ]] || {
    printf 'insufficient disk for artifact installation: %s KiB available, %s KiB required\n' "$available" "$required" >&2
    exit 1
  }
  while IFS='|' read -r service archive digest; do
    [[ -n "$service" ]] || continue
    # Archives are deleted immediately after install so peak disk cost stays at
    # one archive. install_artifacts runs in both the stage and deploy paths, so
    # a missing archive whose digest is already installed is the expected
    # idempotent case, not an error.
    if [[ ! -f "$stage/artifacts/$archive" ]]; then
      if [[ -d "/opt/$service/releases/$digest" ]]; then
        continue
      fi
      printf 'artifact archive does not exist and %s is not installed: %s\n' \
        "$service" "$archive" >&2
      exit 1
    fi
    /usr/local/sbin/install-verified-artifact \
      --archive "$stage/artifacts/$archive" \
      --sha256 "$digest" \
      --service "$service" \
      --root /opt
    rm -f "$stage/artifacts/$archive"
  done < "$stage/artifacts/manifest"
  [[ -x /opt/sigiro/current/sigiro ]]
  [[ -x /opt/node/current/bin/node ]]
  [[ -x /opt/kamal-proxy/current/kamal-proxy ]]
  [[ -f /opt/linguabench/current/server/index.ts ]]
}

install_linguabench_units() {
  local source=/opt/linguabench/current/deploy
  for unit in linguabench-web.service linguabench-worker.service linguabench-analysis.service linguabench-analysis.timer; do
    [[ -f "$source/$unit" ]] || { printf 'LinguaBench artifact is missing %s\n' "$unit" >&2; exit 1; }
    install -m 0644 "$source/$unit" "/etc/systemd/system/$unit"
  done
}

verify_artifacts() {
  local service archive digest current
  while IFS='|' read -r service archive digest; do
    [[ -n "$service" ]] || continue
    current=$(readlink -f "/opt/$service/current")
    [[ "$current" == "/opt/$service/releases/$digest" ]] || {
      printf 'active %s release mismatch: %s\n' "$service" "$current" >&2
      exit 1
    }
  done < "$stage/artifacts/manifest"
}

deploy() {
  require_stage
  install_contract
  install_artifacts
  install_linguabench_units
  verify_artifacts
  systemctl daemon-reload
  systemctl restart sigiro-edge.service sigiro.service kamal-proxy.service
  systemctl enable --now sigiro-ingest-relay.service
  systemctl restart sigiro-ingest-relay.service
  curl --fail --silent --show-error --retry 15 --retry-delay 1 --retry-connrefused http://127.0.0.1:14318/health >/dev/null
  systemctl restart linguabench-web.service linguabench-worker.service
  local unit
  for unit in sigiro-edge sigiro sigiro-ingest-relay linguabench-web linguabench-worker kamal-proxy; do
    systemctl is-active --quiet "$unit.service"
  done
  curl --fail --silent --show-error http://127.0.0.1:9999/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:3001/up >/dev/null
  printf 'native artifacts deployed and verified\n'
}

verify() {
  local unit
  for unit in postgresql@18-main sigiro sigiro-edge sigiro-ingest-relay kamal-proxy kamal-proxy-routes linguabench-web linguabench-worker; do
    systemctl is-active --quiet "$unit.service" || {
      printf 'unit is not active: %s\n' "$unit" >&2
      exit 1
    }
    systemctl is-enabled --quiet "$unit.service" || {
      printf 'unit is not enabled and would not survive reboot: %s\n' "$unit" >&2
      exit 1
    }
  done
  curl --fail --silent --show-error http://127.0.0.1:9999/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:14318/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:3001/up >/dev/null
  # The host runs no container engine: every service is a native systemd unit.
  if command -v docker >/dev/null 2>&1 || command -v containerd >/dev/null 2>&1; then
    printf 'a container engine is installed on the native host\n' >&2; exit 1
  fi
  sudo -u postgres psql -Atc "select current_setting('server_version_num')::int >= 180000" | grep -qx t
  # Health alone passes on an empty catalog, so require Sigiro's tenants.
  [[ "$(sudo -u postgres psql -d sigiro -Atc "select count(*) from sigiro_control.tenants")" -gt 0 ]] || {
    printf 'Sigiro catalog has no tenants; DuckLake telemetry is orphaned\n' >&2
    exit 1
  }
  printf 'native host contract verified\n'
}

case "$command" in
  deploy) deploy ;;
  verify) verify ;;
  *) printf 'usage: %s deploy|verify\n' "$0" >&2; exit 2 ;;
esac
