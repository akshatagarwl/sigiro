import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

export default Alchemy.Stack(
  "SigiroOptimizerSignedDuckLake",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("Signed DuckLake repair requires prod"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const repair = yield* Command.Exec("VerifiedDuckLakeCache", {
      command: "ops/optimizer/repair-signed-ducklake.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: { include: ["ops/optimizer/repair-signed-ducklake.sh"] },
      timeout: "4 minutes",
    });
    return { host, repairHash: repair.hash.input };
  }),
);
