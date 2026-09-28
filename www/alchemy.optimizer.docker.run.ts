import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

// Extends the existing Alchemy-managed Hetzner VM in place. No server or
// firewall resource is created, adopted, or replaced by this stack.
export default Alchemy.Stack(
  "SigiroOptimizerDockerHost",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("Docker host requires the explicit prod stage"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const deployment = yield* Command.Exec("InstallOptimizerDockerEngine", {
      command: "ops/optimizer/deploy-docker-existing.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: { include: [
        "ops/optimizer/deploy-docker-existing.sh",
        "ops/optimizer/install-docker-on-host.sh",
        "ops/optimizer/codex-login.sh",
        "ops/prod/native-host.sh",
      ] },
      timeout: "30 minutes",
    });
    return { host, deploymentHash: deployment.hash.input };
  })
);
