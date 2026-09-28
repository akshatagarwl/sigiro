import { defineAgent } from "eve";
import { experimental_chatgpt } from "eve/models/openai";
export default defineAgent({
  model: experimental_chatgpt(),
  modelContextWindowTokens: 200_000,
  build: {
    externalDependencies: [
      "@rivet-dev/agentos",
      "@rivet-dev/agentos-core",
      "@rivet-dev/agentos-eve",
      "@rivet-dev/agentos-runtime-core",
      "@rivet-dev/agentos-sidecar",
      "@rivet-dev/vercel-world",
      "@rivetkit/engine-cli",
    ],
  },
  experimental: {
    workflow: { world: "#world" },
  },
});
