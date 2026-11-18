#!/usr/bin/env bash
set -euo pipefail

stage=/var/cache/sigiro-native-stage
command=${1:-}
[[ ${EUID:-$(id -u)} -eq 0 ]] || { printf 'must run as root\n' >&2; exit 1; }

require_stage() {
  [[ -d "$stage" ]] || { printf 'native stage is missing: %s\n' "$stage" >&2; exit 1; }
}

ensure_user() {
  local user=$1 home=$2 uid=${3:-}
  if ! id "$user" >/dev/null 2>&1; then
    if [[ -n "$uid" ]]; then
      getent passwd "$uid" >/dev/null && { printf 'required uid %s is already allocated\n' "$uid" >&2; exit 1; }
      useradd --system --uid "$uid" --home-dir "$home" --shell /usr/sbin/nologin --user-group "$user"
    else
      useradd --system --home-dir "$home" --shell /usr/sbin/nologin --user-group "$user"
    fi
  fi
}

install_packages() {
  export DEBIAN_FRONTEND=noninteractive
  systemctl mask nginx.service postgresql.service postgresql@18-main.service >/dev/null 2>&1 || true
  apt-get update
  apt-get install --yes --no-install-recommends ca-certificates curl gnupg nginx-core python3 rsync
  install -d -m 0755 /usr/share/postgresql-common/pgdg
  if [[ ! -s /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc ]]; then
    curl --fail --silent --show-error --location https://www.postgresql.org/media/keys/ACCC4CF8.asc \
      -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
  fi
  # shellcheck disable=SC1091
  printf 'deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt %s-pgdg main\n' \
    "$(. /etc/os-release && printf '%s' "$VERSION_CODENAME")" \
    > /etc/apt/sources.list.d/pgdg.list
  apt-get update
  apt-get install --yes --no-install-recommends postgresql-18 postgresql-18-cron postgresql-18-pgvector postgresql-contrib
  systemctl disable nginx.service >/dev/null 2>&1 || true
  systemctl unmask postgresql.service postgresql@18-main.service >/dev/null
  # `apt-get install` is a no-op when the package is already present, so a cluster
  # whose data directory was removed stays registered but uninitialized and cannot
  # start. Re-initialize in place, keeping /etc/postgresql (certs, sigiro.conf).
  if [ ! -f /var/lib/postgresql/18/main/PG_VERSION ]; then
    install -d -m 0700 -o postgres -g postgres /var/lib/postgresql/18/main
    sudo -u postgres /usr/lib/postgresql/18/bin/initdb \
      --pgdata=/var/lib/postgresql/18/main \
      --encoding=UTF8 --locale=en_US.utf8 --data-checksums
  fi
}

install_contract() {
  install -d -m 0755 /etc/sigiro /etc/linguabench /etc/systemd/system/postgresql@18-main.service.d
  install -d -m 0700 /etc/sigiro/secrets
  install -m 0644 "$stage"/systemd/*.service /etc/systemd/system/
  if compgen -G "$stage/systemd/*.timer" >/dev/null; then
    install -m 0644 "$stage"/systemd/*.timer /etc/systemd/system/
  fi
  install -m 0644 "$stage/environment/sigiro.env" /etc/sigiro/environment
  install -m 0644 "$stage/sigiro-edge.nginx.conf" /etc/sigiro/nginx-edge.conf
  install -m 0755 "$stage/configure-native-routes.sh" /usr/local/sbin/configure-native-routes
  install -m 0755 "$stage/migrate-postgresql18.sh" /usr/local/sbin/migrate-postgresql18
  install -m 0755 "$stage/render-linguabench-environment.py" /usr/local/sbin/render-linguabench-environment
  install -m 0755 "$stage/install-verified-artifact.py" /usr/local/sbin/install-verified-artifact
  install -m 0644 "$stage/postgresql.conf" /etc/postgresql/18/main/sigiro.conf
  install -m 0640 -o postgres -g postgres "$stage/pg_hba.conf" /etc/postgresql/18/main/pg_hba.conf
  grep -qxF "include_if_exists = '/etc/postgresql/18/main/sigiro.conf'" /etc/postgresql/18/main/postgresql.conf \
    || printf "\ninclude_if_exists = '/etc/postgresql/18/main/sigiro.conf'\n" >> /etc/postgresql/18/main/postgresql.conf
  grep -qxF "include_if_exists = '/etc/postgresql/18/main/rehearsal.conf'" /etc/postgresql/18/main/postgresql.conf \
    || printf "include_if_exists = '/etc/postgresql/18/main/rehearsal.conf'\n" >> /etc/postgresql/18/main/postgresql.conf
  install -m 0644 "$stage/secrets/server.crt" /etc/postgresql/18/main/server.crt
  install -m 0600 -o postgres -g postgres "$stage/secrets/server.key" /etc/postgresql/18/main/server.key
  install -m 0644 "$stage/secrets/server.crt" /etc/sigiro/postgres-ca.crt
  install -m 0644 "$stage/secrets/server.crt" /etc/linguabench/postgres-ca.crt
  install -m 0600 "$stage/secrets/sigiro.env" /etc/sigiro/secrets/sigiro.env
  install -m 0600 "$stage/secrets/database.env" /etc/sigiro/secrets/database.env
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

render_linguabench_secrets() {
  local web worker
  web=$(docker ps --format '{{.Names}}' | grep -E '^linguabench-web-' | head -n1)
  worker=$(docker ps --format '{{.Names}}' | grep -E '^linguabench-worker-' | head -n1)
  [[ -n "$web" && -n "$worker" ]] || { printf 'live LinguaBench web/worker containers are required for secret migration\n' >&2; exit 1; }
  docker inspect "$web" > "$stage/linguabench-web.inspect.json"
  docker inspect "$worker" > "$stage/linguabench-worker.inspect.json"
  docker inspect linguabench-analysis > "$stage/linguabench-analysis.inspect.json"
  install -d -m 0700 /var/lib/sigiro-migration
  printf 'LINGUABENCH_WEB=%q\nLINGUABENCH_WORKER=%q\n' "$web" "$worker" \
    > /var/lib/sigiro-migration/docker-rollback.env
  chmod 0600 /var/lib/sigiro-migration/docker-rollback.env
  /usr/local/sbin/render-linguabench-environment --inspect "$stage/linguabench-web.inspect.json" --output /etc/linguabench/web.env
  /usr/local/sbin/render-linguabench-environment --inspect "$stage/linguabench-worker.inspect.json" --output /etc/linguabench/worker.env
  /usr/local/sbin/render-linguabench-environment --inspect "$stage/linguabench-analysis.inspect.json" --output /etc/linguabench/analysis.env
}

# Prove every entrypoint can actually LOAD before cutover depends on it.
# `systemd-analyze verify` only parses unit files; it never executes anything,
# so a broken artifact (missing transitive dep, wrong log target, unusable TLS
# trust store) stages clean and fails only during the downtime window.
smoke_entrypoints() {
  local node=/opt/node/current/bin/node

  # tsx must resolve its own esbuild dependency; an isolated node_modules layout
  # whose symlinks were dereferenced during packaging fails exactly here.
  (cd /opt/linguabench/current && "$node" --import tsx -e 'process.exit(0)') \
    || { printf 'linguabench artifact cannot load tsx/esbuild\n' >&2; exit 1; }

  # Node does not read libpq's sslrootcert, so the CA must be loadable by Node.
  local ca
  for ca in /etc/linguabench/postgres-ca.crt /etc/sigiro/postgres-ca.crt; do
    NODE_EXTRA_CA_CERTS="$ca" "$node" -e 'process.exit(0)' \
      || { printf 'Node cannot load CA bundle %s\n' "$ca" >&2; exit 1; }
  done

  /opt/sigiro/current/sigiro --version >/dev/null \
    || { printf 'sigiro binary does not execute\n' >&2; exit 1; }

  # `--version` exits before any runtime initialisation, so it cannot catch a
  # panic on the hosted boot path. The sync `postgres` client aborts the process
  # from a Tokio thread, which is exactly how a cutover died with SIGABRT while
  # every other check passed. Boot the real unit against the staging cluster and
  # require it to answer /health, then stop it.
  smoke_sigiro_boot

  # nginx cannot open /dev/stderr under systemd's journal socket.
  nginx -t -c /etc/sigiro/nginx-edge.conf \
    || { printf 'sigiro-edge nginx configuration is invalid\n' >&2; exit 1; }

  printf 'entrypoint smoke checks passed\n'
}

# Boot sigiro exactly as the unit does, against the staging PostgreSQL, and
# require a real /health response.
#
# `sigiro --version` returns before any runtime initialisation, so it proves the
# binary links but not that it can boot. The hosted boot path reaches the sync
# `postgres` client, whose internal block_on aborts the process (SIGABRT, not an
# error) when called from a Tokio worker — a cutover died there while every
# other staging check passed.
#
# During staging PostgreSQL 18 listens on the rehearsal port, so the catalog URL
# baked into sigiro.env (port 5432, still owned by Docker) is rewritten here.
smoke_sigiro_boot() {
  local port env_file rehearsal_port log rc=0
  env_file=/etc/sigiro/secrets/sigiro.env
  [[ -r "$env_file" ]] || { printf 'sigiro secrets missing: %s\n' "$env_file" >&2; exit 1; }

  rehearsal_port=5432
  [[ -f /etc/postgresql/18/main/rehearsal.conf ]] \
    && rehearsal_port=$(sed -n 's/^port *= *//p' /etc/postgresql/18/main/rehearsal.conf | tr -d ' ')

  systemctl is-active --quiet postgresql@18-main.service \
    || { printf 'postgresql@18-main is not running; cannot smoke sigiro boot\n' >&2; exit 1; }

  # The staging cluster has no sigiro catalog yet during the first stage, so a
  # failure to CONNECT is not what this checks — it checks the process does not
  # ABORT. Give it a database that exists.
  port=9998
  log=$(mktemp)
  (
    set -a
    # shellcheck source=/dev/null
    . /etc/sigiro/environment
    # shellcheck source=/dev/null
    . "$env_file"
    set +a
    SIGIRO_CATALOG_URL=${SIGIRO_CATALOG_URL//:5432/:$rehearsal_port}
    export SIGIRO_CATALOG_URL
    # Every listener moves: the real ports may already be bound, and a smoke
    # instance must never accept traffic.
    export SIGIRO_HTTP_PORT="$port"
    export SIGIRO_OTLP_HTTP_PORT=9997
    export SIGIRO_GRPC_PORT=9996
    export SIGIRO_DATA_DIR=/var/lib/sigiro-smoke
    install -d -o sigiro -g sigiro -m 0750 /var/lib/sigiro-smoke
    setpriv --reuid sigiro --regid sigiro --init-groups \
      /opt/sigiro/current/sigiro serve >"$log" 2>&1 &
    echo $! > /run/sigiro-smoke.pid
    wait $! 2>/dev/null
  ) &

  local pid ok=no
  for _ in $(seq 1 45); do
    if curl --fail --silent --max-time 2 "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
      ok=yes; break
    fi
    pid=$(cat /run/sigiro-smoke.pid 2>/dev/null)
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then break; fi
    sleep 1
  done

  pid=$(cat /run/sigiro-smoke.pid 2>/dev/null)
  # `|| true` on every kill: the process is usually already gone by the second
  # signal, and a failing kill as the last statement of the function would make
  # the function itself return non-zero under `set -e`.
  if [[ -n "$pid" ]]; then
    kill "$pid" 2>/dev/null || true
    sleep 1
    kill -9 "$pid" 2>/dev/null || true
  fi

  if grep -qiE "Cannot start a runtime from within a runtime|non-unwinding panic|SIGABRT" "$log"; then
    printf 'sigiro aborted during boot (runtime-within-runtime):\n' >&2
    grep -iE "panicked at|Cannot start a runtime" "$log" | head -3 >&2
    rc=1
  elif [[ "$ok" != yes ]]; then
    printf 'sigiro did not answer /health during smoke boot:\n' >&2
    tail -15 "$log" >&2
    rc=1
  fi

  rm -f "$log" /run/sigiro-smoke.pid
  rm -rf /var/lib/sigiro-smoke
  [[ $rc -eq 0 ]] || exit 1
  printf 'sigiro boot smoke passed (answered /health)\n'
}

stage_host() {
  [[ ! -f /var/lib/sigiro-migration/native-active ]] || {
    printf 'native host is already active; use deploy instead of stage\n' >&2
    exit 1
  }
  require_stage
  install_packages
  ensure_user sigiro /var/lib/sigiro
  ensure_user sigiro-edge /var/lib/sigiro-edge
  ensure_user linguabench /var/lib/linguabench
  ensure_user kamal-proxy /var/lib/kamal-proxy 1001
  install -d -m 0750 -o sigiro -g sigiro /var/lib/sigiro/telemetry /var/cache/sigiro/duckdb
  install -d -m 0750 -o linguabench -g linguabench /var/lib/linguabench /var/cache/linguabench
  install -d -m 0750 -o sigiro-edge -g sigiro-edge /run/sigiro-edge
  install -d -m 0750 -o kamal-proxy -g kamal-proxy /var/lib/kamal-proxy
  # Kamal Proxy keeps its route state and TLS certificates under
  # XDG_CONFIG_HOME. Those used to live in a Docker volume, which made the
  # native ingress depend on Docker and put production TLS one
  # `docker volume prune` away from deletion. Migrate them once, preserving the
  # existing certificates so no ACME re-issue is needed.
  install -d -m 0750 -o kamal-proxy -g kamal-proxy /var/lib/kamal-proxy/config
  if [[ -d /var/lib/docker/volumes/kamal-proxy-config/_data ]] \
     && [[ ! -e /var/lib/kamal-proxy/config/certs ]]; then
    cp -a /var/lib/docker/volumes/kamal-proxy-config/_data/. /var/lib/kamal-proxy/config/
    chown -R kamal-proxy:kamal-proxy /var/lib/kamal-proxy/config
    printf 'migrated kamal-proxy config out of the Docker volume\n'
  fi
  install_contract
  printf 'port = 55432\n' > /etc/postgresql/18/main/rehearsal.conf
  install_artifacts
  install_linguabench_units
  render_linguabench_secrets
  systemctl daemon-reload
  systemctl enable postgresql@18-main.service
  systemctl restart postgresql@18-main.service
  systemctl is-active --quiet postgresql@18-main.service
  sudo -u postgres psql -Atc "select current_setting('server_version_num')::int >= 180000" | grep -qx t
  systemd-analyze verify /etc/systemd/system/{sigiro,sigiro-edge,kamal-proxy,linguabench-web,linguabench-worker}.service
  smoke_entrypoints
  printf 'native host staged; Docker workloads remain authoritative\n'
}

deploy() {
  [[ -f /var/lib/sigiro-migration/native-active ]] || {
    printf 'native deployment is disabled until cutover completes\n' >&2
    exit 1
  }
  require_stage
  install_contract
  install_artifacts
  install_linguabench_units
  verify_artifacts
  systemctl daemon-reload
  systemctl restart sigiro-edge.service sigiro.service \
    linguabench-web.service linguabench-worker.service kamal-proxy.service
  local unit
  for unit in sigiro-edge sigiro linguabench-web linguabench-worker kamal-proxy; do
    systemctl is-active --quiet "$unit.service"
  done
  curl --fail --silent --show-error http://127.0.0.1:9999/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:3001/up >/dev/null
  printf 'native artifacts deployed and verified\n'
}

stop_container() {
  local name=$1
  docker update --restart=no "$name" >/dev/null 2>&1 || true
  docker stop --time 30 "$name" >/dev/null 2>&1 || true
}

stop_application_writers() {
  local container
  while IFS= read -r container; do
    # `if` rather than `[[ ]] &&`: a false test on the loop's last iteration
    # would make the function return non-zero and abort cutover under `set -e`.
    if [[ -n "$container" ]]; then
      stop_container "$container"
    fi
  done < <(docker ps --format '{{.Names}}' | grep -E '^(linguabench-(web|worker)-|linguabench-analysis$|linguabench-sigiro-hosted$)' || true)
}

cutover() {
  [[ ! -f /var/lib/sigiro-migration/native-active ]] || { printf 'native cutover already completed\n' >&2; exit 1; }
  [[ -f /var/lib/sigiro-migration/backup-verified ]] || { printf 'verified off-host LinguaBench backup receipt is required\n' >&2; exit 1; }
  stop_application_writers
  /usr/local/sbin/migrate-postgresql18 cutover
  stop_container linguabench-postgres
  stop_container linguabench-sigiro-catalog-db
  rm -f /etc/postgresql/18/main/rehearsal.conf
  systemctl restart postgresql@18-main.service
  systemctl is-active --quiet postgresql@18-main.service

  systemctl enable --now sigiro-edge.service linguabench-web.service linguabench-worker.service
  if [[ -f /etc/systemd/system/linguabench-analysis.timer ]]; then
    systemctl enable --now linguabench-analysis.timer
  fi
  systemctl is-active --quiet sigiro-edge.service
  systemctl is-active --quiet linguabench-web.service
  systemctl is-active --quiet linguabench-worker.service

  stop_container kamal-proxy
  systemctl enable --now kamal-proxy.service
  # Routes go through the oneshot unit, which joins kamal-proxy's PrivateTmp
  # namespace. Invoking the CLI directly from here cannot see the control
  # socket and fails with "dial unix /run/user/0/kamal-proxy.sock".
  systemctl enable --now kamal-proxy-routes.service

  # Sigiro starts LAST, after ingress exists. It fetches the Better Auth JWKS
  # over its PUBLIC issuer URL (https://sigiro.com/api/auth/jwks, now served by
  # the auth Worker on D1) during startup, so it must not race the rest of the
  # cutover. Starting it earlier risks "failed to fetch Better Auth JWKS during
  # startup".
  systemctl enable --now sigiro.service
  systemctl is-active --quiet sigiro.service
  systemctl restart kamal-proxy-routes.service

  stop_container linguabench-sigiro-edge
  stop_container linguabench-registry
  curl --fail --silent --show-error http://127.0.0.1:9999/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:3001/up >/dev/null
  install -m 0600 /dev/null /var/lib/sigiro-migration/native-active
  printf 'native cutover completed; Docker containers and volumes remain as rollback assets\n'
}

verify() {
  local unit
  for unit in postgresql@18-main sigiro sigiro-edge kamal-proxy kamal-proxy-routes linguabench-web linguabench-worker; do
    systemctl is-active --quiet "$unit.service"
  done
  for unit in postgresql@18-main sigiro sigiro-edge kamal-proxy kamal-proxy-routes linguabench-web linguabench-worker; do
    systemctl is-enabled --quiet "$unit.service" || {
      printf 'unit is not enabled and would not survive reboot: %s\n' "$unit" >&2
      exit 1
    }
  done
  curl --fail --silent --show-error http://127.0.0.1:9999/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null
  curl --fail --silent --show-error http://127.0.0.1:3001/up >/dev/null
  # Docker must be absent, not merely idle. The cutover is only complete when
  # the host has no container runtime at all.
  if command -v docker >/dev/null 2>&1; then
    [[ -z "$(docker ps --format '{{.Names}}')" ]] || { printf 'Docker containers are still running\n' >&2; exit 1; }
    printf 'docker is still installed on the host\n' >&2
    exit 1
  fi
  sudo -u postgres psql -Atc "select current_setting('server_version_num')::int >= 180000" | grep -qx t
  # No unit may depend on a Docker path. kamal-proxy previously read its TLS
  # certificates from a Docker volume, which kept Docker load-bearing after the
  # cutover and let `docker volume prune` break production ingress.
  if grep -l '/var/lib/docker' /etc/systemd/system/*.service >/dev/null 2>&1; then
    printf 'a systemd unit still references a Docker path:\n' >&2
    grep -l '/var/lib/docker' /etc/systemd/system/*.service >&2
    exit 1
  fi
  # Sigiro's own state must have survived the migration. Health alone passes on
  # an empty catalog, which is how a destructive cutover previously looked green.
  [[ "$(sudo -u postgres psql -d sigiro -Atc "select count(*) from sigiro_control.tenants")" -gt 0 ]] || {
    printf 'Sigiro catalog has no tenants; DuckLake telemetry is orphaned\n' >&2
    exit 1
  }
  printf 'native host contract verified\n'
}

case "$command" in
  stage) stage_host ;;
  deploy) deploy ;;
  rehearse)
    [[ ! -f /var/lib/sigiro-migration/native-active ]] || { printf 'rehearsal is disabled after native cutover\n' >&2; exit 1; }
    /usr/local/sbin/migrate-postgresql18 rehearsal
    ;;
  cutover) cutover ;;
  verify) verify ;;
  *) printf 'usage: %s stage|deploy|rehearse|cutover|verify\n' "$0" >&2; exit 2 ;;
esac
