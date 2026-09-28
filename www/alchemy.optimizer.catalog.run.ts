import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

export default Alchemy.Stack(
  "SigiroOptimizerDuckLakeCatalogMigration",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("DuckLake catalog migration requires prod"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const migration = yield* Command.Exec("BenchappDuckLakeExactness", {
      command: "ops/optimizer/migrate-ducklake-exactness.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: {
        include: [
          "ops/optimizer/migrate-ducklake-exactness.sh",
          "ops/optimizer/migrate-ducklake-exactness.sql",
        ],
      },
      timeout: "3 minutes",
    });
    return { host, migrationHash: migration.hash.input };
  }),
);
