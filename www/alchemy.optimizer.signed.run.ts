import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

export default Alchemy.Stack(
  "SigiroOptimizerSignedDuckLakeRepository",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(
        new Error("Signed DuckLake installation requires prod")
      );
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const artifact = yield* Config.String("SIGIRO_DUCKLAKE_SIGNED_ARTIFACT");
    const install = yield* Command.Exec("PinnedSignedDuckLakeRepository", {
      command: "ops/optimizer/install-signed-ducklake.sh",
      cwd: "..",
      env: {
        SIGIRO_OPTIMIZER_HOST: host,
        SIGIRO_OPTIMIZER_SSH_KEY: sshKey,
        SIGIRO_DUCKLAKE_SIGNED_ARTIFACT: artifact,
      },
      memo: {
        include: [
          "ops/optimizer/install-signed-ducklake.sh",
          "ops/optimizer/ducklake-signing.pub",
        ],
      },
      timeout: "4 minutes",
    });
    return { host, installHash: install.hash.input };
  })
);
