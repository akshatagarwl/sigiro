import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

// Compose uses host networking, root-only env_file, and a TLS CA bind mount.
// This Alchemy pin's Docker.Container does not expose those settings, so the
// declared resource reconciles the pinned Compose service through Command.Exec.
// The Rivet token and database password remain encrypted in SOPS and are never
// passed as Alchemy properties or emitted in resource outputs.
export default Alchemy.Stack(
  "SigiroOptimizerRivet",
  { providers: Command.providers(), state: CloudflareState.state() },
  Effect.gen(function* () {
    if ((yield* Alchemy.Stack).stage !== "prod") {
      return yield* Effect.die(new Error("Rivet deployment requires the explicit prod stage"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const deployment = yield* Command.Exec("SelfHostedRivet", {
      command: "ops/optimizer/deploy-rivet-existing.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: { include: [
        "ops/optimizer/deploy-rivet-existing.sh",
        "ops/optimizer/provision-rivet.py",
        "ops/optimizer/rivet.compose.yaml",
        "ops/optimizer/secrets.env.sops",
        "ops/prod/pg_hba.conf",
      ] },
      timeout: "30 minutes",
    });
    return { host, deploymentHash: deployment.hash.input };
  })
);
