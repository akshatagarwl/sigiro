import { defineConfig } from "oxfmt";
import core from "ultracite/oxfmt";

export default defineConfig({
  ...core,
  ignorePatterns: [
    ...(core.ignorePatterns ?? []),
    "auth/src/ui.ts",
    "agents/agent/patches/**",
    "www/.astro/**",
    "www/.blume/**",
    "www/dist/**",
    "www/DESIGN.md",
    "www/docs/tutorials/quickstart.mdx",
    "www/docs/*/tutorials/quickstart.mdx",
    "www/openapi.json",
    "www/tools/oxlint/anti-slop/**",
    "www/tsconfig.json",
    "www/wrangler.jsonc",
  ],
});
