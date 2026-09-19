/**
 * One-shot generator for the declarative D1 (SQLite) Drizzle schema.
 *
 * This is NOT wired into CI and NOT part of `aube run check`. Better Auth ships
 * the generator behind a content-hashed internal module filename
 * (`dist/generate-drizzle-schema-<hash>.mjs`) that changes on every release, so
 * a build-time dependency on it would break on each upgrade. Instead we run it
 * by hand, review the diff, and check the result in as
 * `src/schema/auth-schema.ts`.
 *
 * Usage (from `auth/`):
 *   node --import tsx scripts/generate-d1-schema.ts          # print to stdout
 *   node --import tsx scripts/generate-d1-schema.ts --check  # diff vs checked-in file
 *
 * The emitted code is deliberately NOT written straight over the checked-in
 * schema: the generator emits an `authRelations` block built on
 * `defineRelationsPart`, which does not exist in drizzle-orm 0.45.2 (the lower
 * half of Better Auth's own peer range). That block is stripped, and the tables
 * are regrouped by originating plugin for review. See the header of
 * `src/schema/auth-schema.ts`.
 */
import { createRequire } from "node:module";
import path from "node:path";
import { readdirSync } from "node:fs";
import type { Pool } from "pg";

import { createAuthOptions, type AuthRuntime } from "../src/auth.js";

type GenerateDrizzleSchema = (input: {
  options: ReturnType<typeof createAuthOptions>;
  file?: string;
  provider: "pg" | "mysql" | "sqlite";
  camelCase?: boolean;
}) => Promise<{ code: string; fileName: string; path: string }>;

/**
 * Resolve the content-hashed generator module inside @better-auth/drizzle-adapter.
 * The hash changes every release; matching on the stable prefix keeps this script
 * runnable across upgrades while still failing loudly if the module disappears.
 */
const resolveGenerator = async (): Promise<GenerateDrizzleSchema> => {
  const require = createRequire(import.meta.url);
  const adapterEntry = require.resolve("@better-auth/drizzle-adapter");
  const distDir = path.dirname(adapterEntry);
  const candidates = readdirSync(distDir).filter(
    (name) => name.startsWith("generate-drizzle-schema-") && name.endsWith(".mjs"),
  );
  if (candidates.length !== 1) {
    throw new Error(
      `expected exactly one generate-drizzle-schema-*.mjs in ${distDir}, found ${JSON.stringify(candidates)}`,
    );
  }
  const module_ = (await import(path.join(distDir, candidates[0]!))) as {
    generateDrizzleSchema: GenerateDrizzleSchema;
  };
  return module_.generateDrizzleSchema;
};

/**
 * getAuthTables() only reads plugin/table declarations off the options object,
 * so the runtime side (pool, mailer, fetch) is never touched. These stubs exist
 * purely to satisfy the AuthRuntime type without importing a live database.
 */
const schemaOnlyRuntime: AuthRuntime = {
  baseURL: "https://auth.invalid",
  secret: "schema-generation-only",
  resource: "https://api.invalid",
  betterAuthDatabase: {} as Pool,
  runPg: (() => {
    throw new Error("schema generation must not touch the database");
  }) as AuthRuntime["runPg"],
  emailFrom: "schema@invalid",
  sendEmail: () => {
    throw new Error("schema generation must not send email");
  },
};

const main = async (): Promise<void> => {
  const generateDrizzleSchema = await resolveGenerator();
  const result = await generateDrizzleSchema({
    options: createAuthOptions(schemaOnlyRuntime),
    provider: "sqlite",
    camelCase: true,
  });
  process.stdout.write(result.code);
};

await main();
