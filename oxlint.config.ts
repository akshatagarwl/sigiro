import { recommended } from "@effect/tsgo/oxlint-presets";
import { defineConfig } from "oxlint";

const antiSlopRules = {
  "anti-slop/no-chained-type-assertions": "error",
  "anti-slop/no-conditional-empty-object-spread": "error",
  "anti-slop/no-known-value-widening": "error",
  "anti-slop/no-module-mocking": "error",
  "anti-slop/no-object-parameters": "error",
  "anti-slop/no-reflect-apply": "error",
  "anti-slop/no-reflect-get": "error",
  "anti-slop/no-runtime-typeof": "error",
  "anti-slop/no-shape-in-symbol-names": "error",
  "anti-slop/no-unknown-parameters": "error",
  "anti-slop/no-unknown-returns": "error",
  "anti-slop/no-unknown-type-aliases": "error",
  "anti-slop/no-unsafe-dictionary-type": "error",
  "anti-slop/no-widen-then-assert": "error",
  "anti-slop/require-safety-comment-for-type-assertion": "error",
} as const;

export default defineConfig({
  ignorePatterns: [
    "**/node_modules/**",
    "**/dist/**",
    "**/.astro/**",
    "**/.blume/**",
    "**/.cache/**",
    "**/.git/**",
    "target/**",
    "auth/src/ui.ts",
    // shadcn/ui components, copied from the registry and kept as upstream wrote
    // them so `shadcn diff` stays meaningful.
    "www/src/components/ui/**",
    "www/.agent/**",
    "www/.agents/**",
    "www/.claude/**",
    "www/.codex/**",
    "www/.continue/**",
    "www/.cursor/**",
    "www/.gemini/**",
    "www/.letta/**",
    "www/.opencode/**",
    "www/.pi/**",
    "www/.roo/**",
    "www/.windsurf/**",
    "www/tools/oxlint/anti-slop/**",
  ],
  jsPlugins: [
    {
      name: "anti-slop",
      specifier: "./www/tools/oxlint/anti-slop/index.ts",
    },
    {
      name: "policy",
      specifier: "./tools/oxlint/policy.mjs",
    },
  ],
  overrides: [
    {
      files: ["auth/**"],
      plugins: recommended.plugins,
      rules: recommended.rules,
    },
    {
      files: ["www/**"],
      rules: antiSlopRules,
    },
    {
      files: ["cloud/**"],
      rules: {
        "policy/no-comments": "error",
        "policy/no-synthetic-tests": "error",
      },
    },
  ],
});
