import * as Alchemy from 'alchemy';
import { adopt } from 'alchemy/AdoptPolicy';
import * as Cloudflare from 'alchemy/Cloudflare';
import { retain } from 'alchemy/RemovalPolicy';
import * as Effect from 'effect/Effect';
import * as Schema from 'effect/Schema';

const productionStage = Schema.Literal('prod');

const productionResource = <A, R>(resource: Effect.Effect<A, never, R>) =>
  resource.pipe(adopt(true), retain());

export default Alchemy.Stack(
  'SigiroWebsite',
  {
    providers: Cloudflare.providers(),
    state: Cloudflare.state(),
  },
  Effect.gen(function* () {
    const stage = yield* Schema.decodeUnknownEffect(productionStage)(
      (yield* Alchemy.Stack).stage,
    ).pipe(Effect.orDie);

    const zone = yield* productionResource(
      Cloudflare.Zone.Zone('ProductionZone', {
        name: 'sigiro.com',
        type: 'full',
        paused: false,
      }),
    );
    const downloads = yield* productionResource(
      Cloudflare.R2.Bucket('Downloads', {
        name: 'sigiro-dl',
        publicAccess: false,
        forceDestroy: false,
      }),
    );
    const website = yield* productionResource(
      Cloudflare.Worker('Website', {
        name: 'sigiro-www',
        main: './dist/server/blume-worker.mjs',
        compatibility: {
          date: '2026-07-24',
          flags: ['nodejs_compat'],
        },
        workersDev: false,
        observability: { enabled: false },
        assets: {
          directory: './dist/client',
          runWorkerFirst: ['/', '/docs', '/docs/*', '!/docs/*.md', '!/docs/*.mdx'],
        },
        env: { DOWNLOADS: downloads },
      }),
    );
    const apexRoute = yield* productionResource(
      Cloudflare.Workers.WorkerRoute('ApexRoute', {
        zoneId: zone.zoneId,
        pattern: 'sigiro.com/*',
        script: website.workerName,
      }),
    );
    const wwwRoute = yield* productionResource(
      Cloudflare.Workers.WorkerRoute('WwwRoute', {
        zoneId: zone.zoneId,
        pattern: 'www.sigiro.com/*',
        script: website.workerName,
      }),
    );

    return { stage, zone, downloads, website, apexRoute, wwwRoute };
  }),
);
