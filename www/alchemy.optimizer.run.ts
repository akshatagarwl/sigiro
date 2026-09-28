import * as Alchemy from "alchemy";
import * as CloudflareState from "alchemy/Cloudflare/StateStore";
import * as Command from "alchemy/Command";
import * as Config from "effect/Config";
import * as Effect from "effect/Effect";

// Deploys only the optimizer containers, not the existing native Sigiro services.
// An Alchemy plan must be reviewed before the separately authorized deployment.
export default Alchemy.Stack(
  "SigiroOptimizerContainers",
  {
    providers: Command.providers(),
    state: CloudflareState.state(),
  },
  Effect.gen(function* () {
    const stage = (yield* Alchemy.Stack).stage;
    if (stage !== "prod") {
      return yield* Effect.die(new Error("Optimizer containers require the explicit prod stage"));
    }
    const host = yield* Config.String("SIGIRO_OPTIMIZER_HOST");
    const sshKey = yield* Config.String("SIGIRO_OPTIMIZER_SSH_KEY");
    const deployment = yield* Command.Exec("OptimizerDockerCompose", {
      command: "ops/optimizer/deploy-containers.sh",
      cwd: "..",
      env: { SIGIRO_OPTIMIZER_HOST: host, SIGIRO_OPTIMIZER_SSH_KEY: sshKey },
      memo: {
        include: [
          "ops/optimizer/deploy-containers.sh",
          "ops/optimizer/provision-agent-env.py",
          "ops/optimizer/agent.env.sops",
          "ops/optimizer/bridge.env.sops",
          "ops/optimizer/github-app.json.sops",
          "ops/optimizer/secrets.env.sops",
          "ops/prod/secrets.sops.json",
          "agents/agent/Dockerfile",
          "agents/agent/docker-bake.hcl",
          "agents/agent/.dockerignore",
          "agents/agent/compose.yaml",
          "agents/agent/package.json",
          "agents/agent/aube-lock.yaml",
          "agents/agent/actors.ts",
          "agents/agent/world.ts",
          "agents/agent/agent/**",
          "agents/agent/optimizer/**",
        ],
      },
      timeout: "45 minutes",
    });
    return { host, deploymentHash: deployment.hash.input };
  })
);
