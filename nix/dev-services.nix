# Local service stacks, run by process-compose with every tool from nixpkgs
# (no Docker):
#
# * `dev-services`: PostgreSQL 17 with TLS on 127.0.0.1:5432 (user and
#   password `sigiro`, database `ducklake`) and RustFS on 127.0.0.1:9000
#   (console 9001, keys `rustfsadmin`) with the `sigiro` bucket. State lives in
#   `.dev/` (gitignored); delete it to start over.
# * `test-postgres`: a throwaway TLS PostgreSQL on a free port for the
#   PostgreSQL-backed Rust tests, which it runs and then exits with their
#   status. Run it inside `nix develop`, which supplies cargo.
#
# The server certificate comes from mkcert with a CA kept beside it
# (`CAROOT`), not installed in the system trust store: clients pass that CA as
# `sslrootcert`, so `sslmode=verify-full` genuinely verifies, the production
# contract (`storage.rs::validate_hosted_catalog_tls`).
{
  lib,
  writeShellApplication,
  writeText,
  coreutils,
  mkcert,
  rustfs-cli,
  openssl,
  postgresql_17,
  process-compose,
  rustfs,
}:
let
  postgres = postgresql_17;
  pgBin = "${postgres}/bin";
  rc = "${rustfs-cli}/bin/rc";

  # Writes server.crt, server.key and ca.crt to $1, reusing existing ones.
  certs = writeShellApplication {
    name = "sigiro-dev-certs";
    runtimeInputs = [
      coreutils
      mkcert
    ];
    text = ''
      dir=$1
      if [[ -s $dir/ca.crt && -s $dir/server.crt && -s $dir/server.key ]]; then
        echo "certs: reusing $dir"
        exit 0
      fi
      mkdir -p "$dir/ca"
      export CAROOT=$dir/ca
      mkcert -cert-file "$dir/server.crt" -key-file "$dir/server.key" localhost 127.0.0.1 ::1
      cp "$CAROOT/rootCA.pem" "$dir/ca.crt"
      chmod 600 "$dir/server.key" "$CAROOT/rootCA-key.pem"
      chmod 644 "$dir/server.crt" "$dir/ca.crt"
    '';
  };

  # Initialises $PGDATA on first run, then runs PostgreSQL in the foreground
  # with TLS offered (not forced) on $PGPORT, loopback only.
  postgresServer = writeShellApplication {
    name = "sigiro-dev-postgres";
    runtimeInputs = [ coreutils ];
    text = ''
      : "''${PGDATA:?}" "''${PGPORT:?}" "''${PG_USER:?}" "''${PG_PASSWORD:?}" "''${PG_DB:?}" "''${CERT_DIR:?}"
      if [[ ! -s $PGDATA/PG_VERSION ]]; then
        mkdir -p "$PGDATA"
        pwfile=$(mktemp)
        trap 'rm -f "$pwfile"' EXIT
        printf '%s\n' "$PG_PASSWORD" > "$pwfile"
        ${pgBin}/initdb --pgdata="$PGDATA" --username="$PG_USER" --pwfile="$pwfile" \
          --auth-host=scram-sha-256 --auth-local=trust --encoding=UTF8 --no-locale >/dev/null
        if [[ $PG_DB != postgres ]]; then
          echo "CREATE DATABASE \"$PG_DB\";" |
            ${pgBin}/postgres --single -D "$PGDATA" -c unix_socket_directories= postgres >/dev/null
        fi
      fi
      exec ${pgBin}/postgres -D "$PGDATA" -p "$PGPORT" \
        -c listen_addresses=127.0.0.1 -c unix_socket_directories= \
        -c ssl=on -c ssl_cert_file="$CERT_DIR/server.crt" -c ssl_key_file="$CERT_DIR/server.key"
    '';
  };

  rustfsServer = writeShellApplication {
    name = "sigiro-dev-rustfs";
    runtimeInputs = [ coreutils ];
    text = ''
      mkdir -p "$SIGIRO_DEV_DIR/rustfs"
      exec ${lib.getExe rustfs} server --address 127.0.0.1:9000 \
        --console-enable --console-address 127.0.0.1:9001 "$SIGIRO_DEV_DIR/rustfs"
    '';
  };

  postgresProcess = {
    command = lib.getExe postgresServer;
    depends_on.certs.condition = "process_completed_successfully";
    readiness_probe = {
      exec.command = ''${pgBin}/pg_isready -h 127.0.0.1 -p "$PGPORT" -U "$PG_USER" -d "$PG_DB"'';
      period_seconds = 1;
      failure_threshold = 60;
    };
    # PostgreSQL's fast shutdown.
    shutdown.signal = 2;
  };

  certsProcess = {
    command = ''${lib.getExe certs} "$CERT_DIR"'';
  };

  configFile =
    name: processes:
    writeText "${name}.yaml" (
      builtins.toJSON {
        version = "0.5";
        inherit processes;
      }
    );

  devConfig = configFile "sigiro-dev-services" {
    certs = certsProcess;
    postgres = postgresProcess;
    rustfs = {
      command = lib.getExe rustfsServer;
      environment = [
        "RUSTFS_ACCESS_KEY=rustfsadmin"
        "RUSTFS_SECRET_KEY=rustfsadmin"
      ];
      readiness_probe = {
        http_get = {
          host = "127.0.0.1";
          port = 9000;
          path = "/health/ready";
        };
        period_seconds = 1;
        failure_threshold = 60;
      };
    };
    bucket = {
      # RustFS's own client, with its alias file kept in .dev/rc.
      command = ''
        export HOME=$SIGIRO_DEV_DIR/rc XDG_CONFIG_HOME=$SIGIRO_DEV_DIR/rc &&
        mkdir -p "$HOME" &&
        ${rc} alias set local http://127.0.0.1:9000 rustfsadmin rustfsadmin &&
        ${rc} bucket create --ignore-existing local/sigiro
      '';
      depends_on.rustfs.condition = "process_healthy";
    };
  };

  testConfig = configFile "sigiro-test-postgres" {
    certs = certsProcess;
    postgres = postgresProcess;
    tests = {
      command = ''
        cd "$SIGIRO_REPO" &&
        cargo test --locked --test admin_schema_test -- --nocapture &&
        cargo test --locked --lib -- alerts::tests::postgres --nocapture --test-threads=1
      '';
      depends_on.postgres.condition = "process_healthy";
      availability.exit_on_end = true;
    };
  };

  # Plain log output, no TUI and no control port, so stacks can run side by
  # side and in CI; `--tui` on the command line brings the TUI back.
  run = config: ''
    exec ${lib.getExe process-compose} up --config ${config} --no-server --tui=false "$@"
  '';
in
{
  dev-services = writeShellApplication {
    name = "dev-services";
    runtimeInputs = [ coreutils ];
    text = ''
      repo=$(pwd)
      export SIGIRO_DEV_DIR=''${SIGIRO_DEV_DIR:-$repo/.dev}
      export CERT_DIR=$SIGIRO_DEV_DIR/certs
      export PGDATA=$SIGIRO_DEV_DIR/postgres PGPORT=5432
      export PG_USER=sigiro PG_PASSWORD=sigiro PG_DB=ducklake
      echo "catalog: postgresql://sigiro:sigiro@127.0.0.1:5432/ducklake?sslmode=verify-full&sslrootcert=$CERT_DIR/ca.crt"
      echo "objects: http://127.0.0.1:9000 bucket sigiro (rustfsadmin/rustfsadmin)"
      ${run devConfig}
    '';
  };

  test-postgres = writeShellApplication {
    name = "test-postgres";
    runtimeInputs = [
      coreutils
      openssl
    ];
    text = ''
      SIGIRO_REPO=$(pwd)
      export SIGIRO_REPO
      work=$(mktemp -d "''${TMPDIR:-/tmp}/sigiro-test-pg.XXXXXX")
      trap 'rm -rf "$work"' EXIT
      export CERT_DIR=$work/certs PGDATA=$work/data
      export PGPORT=$((20000 + RANDOM % 10000))
      export PG_USER=postgres PG_DB=postgres
      PG_PASSWORD=$(openssl rand -hex 16)
      export PG_PASSWORD
      export SIGIRO_TEST_POSTGRES_URL="postgresql://postgres:$PG_PASSWORD@localhost:$PGPORT/postgres?sslmode=verify-full&sslrootcert=$CERT_DIR/ca.crt"
      ${lib.removePrefix "exec " (run testConfig)}
    '';
  };
}
