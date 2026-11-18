import { defineAgent, defineDynamic } from "eve";
import { subscriptionModel, summaryModel } from "./lib/subscription-model.ts";

export default defineAgent({
  experimental: {
    instrumentationProviders: true,
    workflow: { world: "@ewhauser/world-celld" },
  },
  build: { externalDependencies: ["@ewhauser/world-celld", "ai-sdk-provider-claude-code", "@anthropic-ai/claude-agent-sdk"] },
  model: defineDynamic({ events: {
    "step.started": (_event, ctx) => {
      if (ctx.channel.metadata?.prepared !== true || ctx.channel.metadata?.error !== null) {
        throw new Error(`Celld workspace is not ready: ${ctx.channel.metadata?.error ?? "no prepared job"}`);
      }
      return { model: subscriptionModel(ctx.session.id), modelContextWindowTokens: 200_000 };
    },
  } }),
  compaction: { model: summaryModel(), modelContextWindowTokens: 200_000 },
  limits: { maxOutputTokensPerSession: 12_000 },
});
