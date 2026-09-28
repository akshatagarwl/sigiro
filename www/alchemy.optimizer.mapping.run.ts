import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

export default Alchemy.Stack(
  "SigiroOptimizerWorkerRepositoryMapping",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("Worker mapping migration requires prod"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const migration = yield* Command.Exec("MapRealWorkerFinding", {
      command: "ops/optimizer/map-worker-service.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: { include: ["ops/optimizer/map-worker-service.sh", "ops/optimizer/map-worker-service.sql"] },
      timeout: "3 minutes",
    });
    return { host, migrationHash: migration.hash.input };
  }),
);
