import { defineConfig } from "oxfmt";
import core from "ultracite/oxfmt";

export default defineConfig({
  ...core,
  ignorePatterns: [
    ...(core.ignorePatterns ?? []),
    "auth/src/ui.ts",
    "www/.astro/**",
    "www/.blume/**",
    "www/dist/**",
    "www/docs/tutorials/quickstart.mdx",
    "www/docs/*/tutorials/quickstart.mdx",
    "www/openapi.json",
    "www/tools/oxlint/anti-slop/**",
    "www/tsconfig.json",
    "www/wrangler.jsonc",
    // ../dashboard: code installed from shadcn registries (shadcn/ui,
    // dashboard-01, openstatus data-table-filters, EvilCharts, assistant-ui),
    // kept as the registries ship it so `shadcn add --diff` stays meaningful.
    // Our own code there is dashboard/app, components/sigiro and lib/sigiro.
    "dashboard/.next/**",
    "dashboard/next-env.d.ts",
    "dashboard/components/ui/**",
    "dashboard/components/custom/**",
    "dashboard/components/data-table/**",
    "dashboard/components/evilcharts/**",
    "dashboard/components/assistant-ui/**",
    "dashboard/components/*.tsx",
    "dashboard/hooks/**",
    "dashboard/lib/*.ts",
    "dashboard/lib/actions/**",
    "dashboard/lib/constants/**",
    "dashboard/lib/data-table/**",
    "dashboard/lib/filters/**",
    "dashboard/lib/request/**",
    "dashboard/lib/store/**",
    "dashboard/lib/table/**",
    "dashboard/lib/table-schema/**",
  ],
});
