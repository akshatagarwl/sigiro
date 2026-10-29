import * as Alchemy from "alchemy";
import { adopt } from "alchemy/AdoptPolicy";
import * as Cloudflare from "@alchemy.run/cloudflare";
import { retain } from "alchemy/RemovalPolicy";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";

const productionStage = Schema.Literal("prod");

const productionResource = <A, R>(resource: Effect.Effect<A, never, R>) =>
  resource.pipe(adopt(true), retain());

export default Alchemy.Stack(
  "SigiroWebsite",
  {
    providers: Cloudflare.providers(),
    state: Cloudflare.state(),
  },
  Effect.gen(function* () {
    const stage = yield* Schema.decodeUnknownEffect(productionStage)(
      (yield* Alchemy.Stack).stage
    ).pipe(Effect.orDie);

    // The same secret the Node service used. A rotated BETTER_AUTH_SECRET is
    // indistinguishable from a key re-mint and would invalidate every token.
    const authSecret = yield* Config.Redacted("BETTER_AUTH_SECRET");
    const emailToken = yield* Config.Redacted("CLOUDFLARE_EMAIL_API_TOKEN");

    const zone = yield* productionResource(
      Cloudflare.Zone.Zone("ProductionZone", {
        name: "sigiro.com",
        type: "full",
        paused: false,
      })
    );
    const downloads = yield* productionResource(
      Cloudflare.R2.Bucket("Downloads", {
        name: "sigiro-dl",
        publicAccess: false,
        forceDestroy: false,
      })
    );
    const emailSending = yield* productionResource(
      Cloudflare.Email.SendingSubdomain("TransactionalEmail", {
        zoneId: zone.zoneId,
        name: "sigiro.com",
      })
    );
    const authRecord = yield* productionResource(
      Cloudflare.DNS.Record("AuthRecord", {
        zoneId: zone.zoneId,
        name: "auth.sigiro.com",
        type: "A",
        content: "203.0.113.10",
        proxied: false,
        ttl: 60,
      })
    );
    const apiRecord = yield* productionResource(
      Cloudflare.DNS.Record("ApiRecord", {
        zoneId: zone.zoneId,
        name: "api.sigiro.com",
        type: "A",
        content: "203.0.113.10",
        proxied: false,
        ttl: 60,
      })
    );
    // The auth service runs as a Worker on D1. The website reaches it through a
    // service binding below, so there is no auth-origin DNS record, no tunnel,
    // and no public hop: auth is no longer addressable from the internet except
    // through sigiro.com/api/auth.
    const authDatabase = yield* productionResource(
      Cloudflare.D1.Database("AuthDatabase", {
        name: "sigiro-auth",
      })
    );
    const authWorker = yield* productionResource(
      Cloudflare.Workers.Worker("AuthWorker", {
        name: "sigiro-auth",
        main: "../auth/src/d1/worker.ts",
        compatibility: {
          date: "2026-09-01",
          flags: ["nodejs_compat"],
        },
        workersDev: false,
        observability: { enabled: true },
        env: {
          DB: authDatabase,
          ENVIRONMENT: "production",
          BETTER_AUTH_URL: "https://sigiro.com",
          SIGIRO_RESOURCE: "https://sigiro.com",
          EMAIL_FROM: "noreply@sigiro.com",
          CLOUDFLARE_ACCOUNT_ID: "73035685fa8536f5c07b55b168868c71",
          BETTER_AUTH_SECRET: authSecret,
          CLOUDFLARE_EMAIL_API_TOKEN: emailToken,
        },
      })
    );
    const apiOriginRecord = yield* productionResource(
      Cloudflare.DNS.Record("ApiOriginRecord", {
        zoneId: zone.zoneId,
        name: "api-origin.sigiro.com",
        type: "A",
        content: "203.0.113.10",
        proxied: false,
        ttl: 60,
      })
    );

    const website = yield* productionResource(
      Cloudflare.Worker("Website", {
        name: "sigiro-www",
        main: "./src/worker.ts",
        compatibility: {
          date: "2026-07-24",
          flags: ["nodejs_compat"],
        },
        workersDev: false,
        observability: { enabled: false },
        assets: {
          directory: "./dist/client",
          runWorkerFirst: [
            "/",
            "/docs",
            "/docs/*",
            "!/docs/*.md",
            "!/docs/*.mdx",
            "/.well-known/agent-configuration",
            "/.well-known/oauth-protected-resource",

            "/.well-known/mcp.json",
            "/.well-known/mcp/*",
            "/api/auth",
            "/api/auth/*",
            "/auth-ui.css",
            "/auth-ui.js",
            "/consent",
            "/consent/*",
            "/device",
            "/device/*",
            "/health",
            "/healthz",
            "/ingest",
            "/ingest/*",
            "/openapi.json",
            "/push.v1.PusherService/Push",
            "/sign-in",
            "/sign-in/*",
            "/sign-up",
            "/sign-up/*",
            "/v1/*",
            "/mcp",
            "/v1development/profiles",
          ],
        },
        env: {
          API_ORIGIN_URL: "https://api-origin.sigiro.com",
          // A service binding, not a URL: the request goes worker-to-worker
          // without leaving Cloudflare, so auth has no public origin to attack
          // and no tunnel to keep alive.
          AUTH: authWorker,
          DOWNLOADS: downloads,
        },
      })
    );
    const apexRoute = yield* productionResource(
      Cloudflare.Workers.WorkerRoute("ApexRoute", {
        zoneId: zone.zoneId,
        pattern: "sigiro.com/*",
        script: website.workerName,
      })
    );
    const wwwRoute = yield* productionResource(
      Cloudflare.Workers.WorkerRoute("WwwRoute", {
        zoneId: zone.zoneId,
        pattern: "www.sigiro.com/*",
        script: website.workerName,
      })
    );

    return {
      stage,
      zone,
      downloads,
      emailSending,
      authRecord,
      apiRecord,
      apiOriginRecord,
      authDatabase,
      authWorker,

      website,
      apexRoute,
      wwwRoute,
    };
  })
);
