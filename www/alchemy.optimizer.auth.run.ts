import * as Alchemy from "alchemy";
import { adopt } from "alchemy/AdoptPolicy";
import * as Cloudflare from "alchemy/Cloudflare";
import { retain } from "alchemy/RemovalPolicy";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

// Auth-only rollout: changing the OAuth scope must not redeploy the website's
// unrelated static assets. The main SigiroWebsite stack declares this same
// Worker; future website rollouts reconcile the shared source configuration.
export default Alchemy.Stack(
  "SigiroOptimizerAuthScope",
  { providers: Cloudflare.providers(), state: Cloudflare.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("Optimizer auth scope requires prod"));
    }
    const authSecret = yield* Config.Redacted("BETTER_AUTH_SECRET");
    const emailToken = yield* Config.Redacted("CLOUDFLARE_EMAIL_API_TOKEN");
    const db = yield* Cloudflare.D1.Database("ExistingAuthDatabase", {
      name: "sigiro-auth",
    }).pipe(adopt(true), retain());
    const worker = yield* Cloudflare.Workers.Worker("ExistingAuthWorker", {
      name: "sigiro-auth",
      main: "../auth/src/d1/worker.ts",
      compatibility: { date: "2026-09-01", flags: ["nodejs_compat"] },
      workersDev: false,
      observability: { enabled: true },
      env: {
        DB: db,
        ENVIRONMENT: "production",
        BETTER_AUTH_URL: "https://sigiro.com",
        SIGIRO_RESOURCE: "https://sigiro.com",
        EMAIL_FROM: "noreply@sigiro.com",
        CLOUDFLARE_ACCOUNT_ID: "73035685fa8536f5c07b55b168868c71",
        BETTER_AUTH_SECRET: authSecret,
        CLOUDFLARE_EMAIL_API_TOKEN: emailToken,
      },
    }).pipe(adopt(true), retain());
    return { workerName: worker.name };
  }),
);
