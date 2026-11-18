import * as Alchemy from 'alchemy';
import * as CloudflareState from 'alchemy/Cloudflare/StateStore';
import * as Command from 'alchemy/Command';
import * as Docker from 'alchemy/Docker';
import * as Config from 'effect/Config';
import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';

export default Alchemy.Stack(
  'SigiroProductionWorkload',
  {
    providers: Layer.mergeAll(Docker.providers(), Command.providers()),
    state: CloudflareState.state(),
  },
  Effect.gen(function* () {
    const host = yield* Config.String('SIGIRO_HOST').pipe(Config.withDefault('root@91.98.118.235'));
    const sshKey = yield* Config.String('SIGIRO_SSH_KEY').pipe(
      Config.withDefault('~/.ssh/linguabench'),
    );
    const action = yield* Config.String('SIGIRO_NATIVE_ACTION').pipe(Config.withDefault('stage'));
    const backupReceipt = yield* Config.String('LINGUABENCH_BACKUP_RECEIPT').pipe(
      Config.withDefault(''),
    );

    const artifact = yield* Config.String('SIGIRO_ARTIFACT');
    const artifactSha256 = yield* Config.String('SIGIRO_ARTIFACT_SHA256');
    const nodeArtifact = yield* Config.String('NODE_ARTIFACT');
    const nodeArtifactSha256 = yield* Config.String('NODE_ARTIFACT_SHA256');
    const proxyArtifact = yield* Config.String('KAMAL_PROXY_ARTIFACT');
    const proxyArtifactSha256 = yield* Config.String('KAMAL_PROXY_ARTIFACT_SHA256');
    const linguabenchArtifact = yield* Config.String('LINGUABENCH_ARTIFACT');
    const linguabenchArtifactSha256 = yield* Config.String('LINGUABENCH_ARTIFACT_SHA256');

    const catalogPassword = yield* Config.Redacted('SIGIRO_CATALOG_PASSWORD');
    // Despite the name, SIGIRO_AUTH_DB_TLS_* is the PostgreSQL server
    // certificate and key for the LIVE catalog cluster, not auth material.
    const databaseCertificate = yield* Config.Redacted('SIGIRO_AUTH_DB_TLS_CERT_B64');
    const databaseKey = yield* Config.Redacted('SIGIRO_AUTH_DB_TLS_KEY_B64');
    const adminToken = yield* Config.Redacted('SIGIRO_ADMIN_TOKEN');

    const deployment = yield* Command.Exec('ProductionNativeSystemd', {
      command: 'ops/prod/deploy-native-host.sh',
      cwd: '..',
      env: {
        SIGIRO_HOST: host,
        SIGIRO_SSH_KEY: sshKey,
        SIGIRO_NATIVE_ACTION: action,
        LINGUABENCH_BACKUP_RECEIPT: backupReceipt,
        SIGIRO_ARTIFACT: artifact,
        SIGIRO_ARTIFACT_SHA256: artifactSha256,
        NODE_ARTIFACT: nodeArtifact,
        NODE_ARTIFACT_SHA256: nodeArtifactSha256,
        KAMAL_PROXY_ARTIFACT: proxyArtifact,
        KAMAL_PROXY_ARTIFACT_SHA256: proxyArtifactSha256,
        LINGUABENCH_ARTIFACT: linguabenchArtifact,
        LINGUABENCH_ARTIFACT_SHA256: linguabenchArtifactSha256,
        SIGIRO_CATALOG_PASSWORD: catalogPassword,
        SIGIRO_AUTH_DB_TLS_CERT_B64: databaseCertificate,
        SIGIRO_AUTH_DB_TLS_KEY_B64: databaseKey,
        SIGIRO_ADMIN_TOKEN: adminToken,
      },
      memo: {
        include: [
          'ops/prod/deploy-native-host.sh',
          'ops/prod/native-host.sh',
          'ops/prod/configure-native-routes.sh',
          'ops/prod/migrate-postgresql18.sh',
          'ops/prod/environment/**',
          'ops/prod/systemd/**',
          'ops/prod/postgresql.conf',
          'ops/prod/pg_hba.conf',
          'scripts/install-verified-artifact.py',
          'scripts/render-native-secrets.py',
          'scripts/render-linguabench-environment.py',
          'scripts/rollback-native-host.sh',
        ],
      },
      timeout: '60 minutes',
    });

    return {
      controller: 'alchemy-command-exec',
      host,
      action,
      artifacts: {
        sigiro: artifactSha256,
        node: nodeArtifactSha256,
        proxy: proxyArtifactSha256,
        linguabench: linguabenchArtifactSha256,
      },
      deployment: deployment.stdout,
    };
  }),
);
