import type { SecondaryStorage } from "@better-auth/core/db";
import { Effect, Schema } from "effect";

import { query, type RunPg } from "./database.js";

const TABLE = "better_auth_secondary_storage";
const ValueRow = Schema.Struct({ value: Schema.String });
const ValueRows = Schema.Array(ValueRow);
const CounterRow = Schema.Struct({
  value: Schema.NumberFromString.check(
    Schema.isInt(),
    Schema.makeFilter((value: number) =>
      Number.isSafeInteger(value) || "secondary-storage counter must be a safe integer"),
  ),
});
const CounterRows = Schema.Array(CounterRow);

const decodeValue = Effect.fnUntraced(function* (rows: ReadonlyArray<unknown>) {
  const decoded = yield* Schema.decodeUnknownEffect(ValueRows)(rows);
  return decoded[0]?.value ?? null;
});

const get = Effect.fn("SecondaryStorage.get")(function* (key: string) {
  return yield* decodeValue(yield* query(
    `SELECT value FROM ${TABLE} WHERE key = $1 AND (expires_at IS NULL OR expires_at > now())`,
    [key],
  ));
});

const getAndDelete = Effect.fn("SecondaryStorage.getAndDelete")(function* (key: string) {
  return yield* decodeValue(yield* query(
    `DELETE FROM ${TABLE}
     WHERE key = $1 AND (expires_at IS NULL OR expires_at > now())
     RETURNING value`,
    [key],
  ));
});

const increment = Effect.fn("SecondaryStorage.increment")(function* (key: string, ttl: number) {
  const rows = yield* query(
    `INSERT INTO ${TABLE} (key, value, expires_at)
     VALUES ($1, '1', now() + make_interval(secs => $2))
     ON CONFLICT (key) DO UPDATE SET
       value = CASE
         WHEN ${TABLE}.expires_at IS NOT NULL AND ${TABLE}.expires_at <= now() THEN '1'
         ELSE ((${TABLE}.value::bigint + 1)::text)
       END,
       expires_at = CASE
         WHEN ${TABLE}.expires_at IS NOT NULL AND ${TABLE}.expires_at <= now() THEN EXCLUDED.expires_at
         ELSE ${TABLE}.expires_at
       END
     RETURNING value`,
    [key, ttl],
  );
  const decoded = yield* Schema.decodeUnknownEffect(CounterRows)(rows);
  const value = decoded[0]?.value;
  if (value === undefined) return yield* Effect.die(new Error(`secondary-storage increment returned no value for ${key}`));
  return value;
});

const set = Effect.fn("SecondaryStorage.set")(function* (key: string, value: string, ttl?: number) {
  yield* query(
    `INSERT INTO ${TABLE} (key, value, expires_at)
     VALUES ($1, $2, CASE WHEN $3::double precision IS NULL THEN NULL ELSE now() + make_interval(secs => $3) END)
     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, expires_at = EXCLUDED.expires_at`,
    [key, value, ttl ?? null],
  );
});

const remove = Effect.fn("SecondaryStorage.delete")(function* (key: string) {
  yield* query(`DELETE FROM ${TABLE} WHERE key = $1`, [key]);
});

export const migrateSecondaryStorage = Effect.fn("migrateSecondaryStorage")(function* () {
  yield* query(`
    CREATE TABLE IF NOT EXISTS ${TABLE} (
      key text PRIMARY KEY,
      value text NOT NULL,
      expires_at timestamptz
    )
  `);
  yield* query(`
    CREATE INDEX IF NOT EXISTS better_auth_secondary_storage_expires_at_idx
    ON ${TABLE} (expires_at)
    WHERE expires_at IS NOT NULL
  `);
  yield* query(`DELETE FROM ${TABLE} WHERE expires_at <= now()`);
});

export const postgresSecondaryStorage = (run: RunPg): SecondaryStorage => ({
  get: (key) => run(get(key)),
  getAndDelete: (key) => run(getAndDelete(key)),
  increment: (key, ttl) => run(increment(key, ttl)),
  set: (key, value, ttl) => run(set(key, value, ttl)),
  delete: (key) => run(remove(key)),
});
