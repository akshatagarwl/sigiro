import * as Alchemy from 'alchemy';
import * as Command from 'alchemy/Command';
import * as Docker from 'alchemy/Docker';
import { localState } from 'alchemy/State/LocalState';
import * as Config from 'effect/Config';
import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';

/**
 * The LOCAL development stack: PostgreSQL (with TLS), RustFS, and the `sigiro`
 * bucket the S3-backed storage mode writes to.
 *
 * This replaced the hand-rolled `docker-compose.dev.yml`, now deleted. Production is already
 * Alchemy (`alchemy.run.ts`, `alchemy.hetzner.run.ts`,
 * `alchemy.workload.run.ts`); local dev was the last hand-written surface, and
 * because it was written in a different language it drifted from production
 * silently. A config-level mistake that only production could catch costs a
 * cross-compile, a 287MB upload, a systemd restart, a journald read, and a
 * rollback — roughly fifteen minutes. Expressed here it is a seconds-long local
 * failure in the same vocabulary production uses.
 *
 * State is LOCAL (`localState()`), not `Cloudflare.state()`: this stack's
 * resources are ephemeral containers on one developer's machine. Pushing their
 * state to the shared remote store would let one laptop's `destroy` plan
 * against another's containers.
 */
export default Alchemy.Stack(
  'SigiroLocalDev',
  {
    providers: Layer.mergeAll(Docker.providers(), Command.providers()),
    state: localState(),
  },
  Effect.gen(function* () {
    // Fixed physical names. Alchemy would otherwise derive names from
    // stack+stage+id, but the Rust E2Es connect to `127.0.0.1:5432` and
    // `127.0.0.1:9000` by hardcoded constant, and the bucket bootstrap needs a
    // stable DNS alias on the shared network.
    const networkName = 'sigiro-dev';
    const postgresName = 'sigiro-dev-postgres';
    const rustfsName = 'sigiro-dev-rustfs';

    // `scripts/dev-certs.sh` writes these (mkcert, gitignored). Absolute,
    // because the container bind-mounts the directory and Docker rejects a
    // relative source. The repository root is the parent of `www/`.
    const repositoryRoot = yield* Config.String('SIGIRO_DEV_REPO_ROOT').pipe(
      Config.withDefault(new URL('..', import.meta.url).pathname.replace(/\/$/, '')),
    );
    const certificateDirectory = `${repositoryRoot}/.dev/certs`;

    const network = yield* Docker.Network('DevNetwork', {
      name: networkName,
      driver: 'bridge',
    });

    const postgresData = yield* Docker.Volume('PostgresData', {
      name: 'sigiro-dev-pg-data',
    });
    const rustfsData = yield* Docker.Volume('RustfsData', {
      name: 'sigiro-dev-rustfs-data',
    });

    // TLS is not optional dressing: `storage.rs::validate_hosted_catalog_tls`
    // refuses any hosted catalog URL that is not `sslmode=verify-full` with an
    // `sslrootcert`, so a plaintext dev Postgres cannot exercise the hosted
    // storage path at all.
    //
    // The key has to be 0600 and owned by `postgres` INSIDE the container,
    // which a read-only host bind cannot provide — hence the copy from a
    // read-only `/certs-src` mount into a container-owned `/certs`. Compose
    // spelled this as `entrypoint:`; the pinned Alchemy Docker provider has no
    // `entrypoint` prop (see `ContainerProps`), only `command`. That is enough:
    // `docker-entrypoint.sh` ends in `exec "$@"`, so a `bash -c` command is
    // still run by it — the same script, reached through the image's own
    // entrypoint instead of replacing it.
    //
    // Non-TLS clients still connect: `ssl=on` OFFERS TLS, pg_hba does not
    // force it.
    const postgres = yield* Docker.Container('Postgres', {
      name: postgresName,
      image: 'postgres:17',
      command: [
        'bash',
        '-c',
        [
          'set -euo pipefail',
          'install -d -o postgres -g postgres -m 0750 /certs',
          'install -o postgres -g postgres -m 0644 /certs-src/server.crt /certs/server.crt',
          'install -o postgres -g postgres -m 0600 /certs-src/server.key /certs/server.key',
          'exec docker-entrypoint.sh postgres' +
            ' -c ssl=on' +
            ' -c ssl_cert_file=/certs/server.crt' +
            ' -c ssl_key_file=/certs/server.key',
        ].join('\n'),
      ],
      environment: {
        POSTGRES_DB: 'ducklake',
        POSTGRES_USER: 'sigiro',
        POSTGRES_PASSWORD: 'sigiro',
      },
      ports: [{ external: 5432, internal: 5432 }],
      volumes: [
        { hostPath: postgresData.name, containerPath: '/var/lib/postgresql/data' },
        { hostPath: certificateDirectory, containerPath: '/certs-src', readOnly: true },
      ],
      networks: [{ name: network.name, aliases: ['postgres'] }],
      healthcheck: {
        cmd: ['CMD-SHELL', 'pg_isready -U sigiro'],
        interval: '5 seconds',
        retries: 5,
      },
      start: true,
    });

    const rustfs = yield* Docker.Container('Rustfs', {
      name: rustfsName,
      // Pinned by digest, exactly as the retired compose file pinned it.
      image:
        'rustfs/rustfs:1.0.0-rc.5@sha256:c36b3efea3d1e503f1a2581abd0e7611e0e5820dd30e1850a52384b3fc52bda4',
      command: ['/data'],
      environment: {
        RUSTFS_ACCESS_KEY: 'rustfsadmin',
        RUSTFS_SECRET_KEY: 'rustfsadmin',
      },
      ports: [
        { external: 9000, internal: 9000 },
        { external: 9001, internal: 9001 },
      ],
      volumes: [{ hostPath: rustfsData.name, containerPath: '/data' }],
      networks: [{ name: network.name, aliases: ['rustfs'] }],
      healthcheck: {
        cmd: ['CMD', 'curl', '-f', 'http://localhost:9000/health/ready'],
        interval: '5 seconds',
        retries: 5,
      },
      start: true,
    });

    // `Docker.Container` reconciles once the container is STARTED — the
    // healthcheck it configures is observed by Docker, not awaited by the
    // deploy, and the pinned provider has no `depends_on: service_healthy`
    // equivalent. So readiness is an explicit resource: without it `alchemy
    // deploy` would report success while the first client still raced initdb.
    const postgresReady = yield* Command.Exec('PostgresReady', {
      command: 'scripts/dev-wait-postgres.sh',
      cwd: '..',
      env: {
        SIGIRO_DEV_POSTGRES_CONTAINER: postgres.name,
        SIGIRO_DEV_POSTGRES_USER: 'sigiro',
        SIGIRO_DEV_POSTGRES_DB: 'ducklake',
      },
      // Readiness is a property of the RUNNING container, not of any file on
      // disk, so content-hash memoization would skip the wait on the very
      // deploys that need it (a re-deploy after `docker rm`).
      memo: false,
      timeout: '2 minutes',
    });

    // Replaces the one-shot `rustfs-setup` compose service: same `rc alias set`
    // + `rc bucket create --ignore-existing`, same pinned image, but the retry
    // loop now also serves as RustFS's readiness gate.
    const bucket = yield* Command.Exec('Bucket', {
      command: 'scripts/dev-rustfs-bucket.sh',
      cwd: '..',
      env: {
        SIGIRO_DEV_NETWORK: network.name,
        // The `rc` container joins the same network and resolves RustFS by its
        // network alias. `rustfs.name` is threaded through purely so Alchemy
        // orders this AFTER the container exists — the script runs `docker run`
        // itself, so there is no other data edge to infer the ordering from.
        SIGIRO_DEV_RUSTFS_CONTAINER: rustfs.name,
        SIGIRO_DEV_RUSTFS_ENDPOINT: 'http://rustfs:9000',
        SIGIRO_DEV_BUCKET: 'sigiro',
        SIGIRO_DEV_RUSTFS_ACCESS_KEY: 'rustfsadmin',
        SIGIRO_DEV_RUSTFS_SECRET_KEY: 'rustfsadmin',
      },
      memo: false,
      timeout: '5 minutes',
    });

    return {
      network: network.name,
      postgres: {
        container: postgres.name,
        // What the E2Es and `mise run run` connect to. `sslmode=verify-full`
        // with `sslrootcert` is the production contract, not decoration.
        catalogUrl:
          'postgresql://sigiro:***@127.0.0.1:5432/ducklake' +
          `?sslmode=verify-full&sslrootcert=${certificateDirectory}/ca.crt`,
        ready: postgresReady.hash.input,
      },
      rustfs: {
        container: rustfs.name,
        endpoint: 'http://127.0.0.1:9000',
        bucket: 'sigiro',
        bucketReady: bucket.hash.input,
      },
      volumes: {
        postgres: postgresData.name,
        rustfs: rustfsData.name,
      },
    };
  }),
);
