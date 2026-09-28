import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

export default Alchemy.Stack(
  "SigiroOptimizerHistoricalWorkerTelemetry",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("Historical telemetry backfill requires prod"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const backfill = yield* Command.Exec("AuthenticWorkerJournalBackfill", {
      command: "ops/optimizer/backfill-worker-journal.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: { include: ["ops/optimizer/backfill-worker-journal.sh", "ops/optimizer/backfill-worker-journal.py"] },
      timeout: "8 minutes",
    });
    return { host, backfillHash: backfill.hash.input };
  }),
);
