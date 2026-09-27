import { schemaCheckFor } from "@better-auth/core/db/internal";
/**
 * Startup schema-parity guard for the D1 auth database.
 *
 * Better Auth's Drizzle adapter registers a schema check whenever it is handed
 * a `schema` object (see `registerSchemaCheck` / `findDrizzleSchemaProblems` in
 * @better-auth/drizzle-adapter). The check compares the checked-in Drizzle
 * schema in `./auth-schema.js` against `getAuthTables(options)` — the tables the
 * configured plugin set actually writes to — and reports missing tables, missing
 * columns, and nullability/default mismatches.
 *
 * Why this matters: without it, a table or column that the schema artifact is
 * missing is not an error. The adapter simply issues a query against something
 * that is not there, and the failure surfaces much later as an auth request that
 * mysteriously does not work. This converts that into a loud failure at boot.
 *
 * Better Auth runs the registered check itself during context creation, so
 * building the adapter through {@link authDrizzleAdapter} is sufficient to get
 * the guard. {@link assertAuthSchemaMatches} exposes the same check directly so
 * tests — and a deploy-time preflight — can force it without standing up a
 * request.
 */
import { drizzleAdapter, type DB } from "@better-auth/drizzle-adapter";
import type { BetterAuthOptions } from "better-auth";

import * as authSchema from "./auth-schema.js";

/** The checked-in declarative schema, as the adapter addresses it. */
export const schema = authSchema;

/**
 * Build the Drizzle adapter for D1 with the checked-in schema attached.
 *
 * `camelCase: true` matches how the schema was generated: Better Auth's field
 * names are used verbatim as column names, with no snake_case conversion.
 * Passing `schema` is what arms the startup parity check.
 *
 * `schemaOverride` exists so tests can point the same guard at a deliberately
 * damaged schema; production callers omit it.
 */
export const authDrizzleAdapter = (
  database: DB,
  schemaOverride: Record<string, unknown> = schema
) =>
  drizzleAdapter(database, {
    provider: "sqlite",
    schema: schemaOverride,
    camelCase: true,
  });

/**
 * Run the registered schema check and throw `SchemaMismatchError` if the
 * checked-in schema has drifted from what the configured plugins expect.
 *
 * Resolves cleanly when the schema matches. The underlying check caches a clean
 * verdict, so calling this repeatedly is cheap.
 */
export const assertAuthSchemaMatches = async (
  options: BetterAuthOptions,
  database: DB = {},
  schemaOverride: Record<string, unknown> = schema
): Promise<void> => {
  const adapter = authDrizzleAdapter(database, schemaOverride)(options);
  const check = schemaCheckFor(adapter);
  if (!check) {
    throw new Error(
      "drizzle adapter registered no schema check — the guard is not armed; " +
        "confirm advanced.database.validateSchema is not disabled"
    );
  }
  await check();
};
