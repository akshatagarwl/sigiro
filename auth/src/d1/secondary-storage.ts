import type { SecondaryStorage } from "@better-auth/core/db";
import type { D1Database } from "@cloudflare/workers-types";

/**
 * D1-backed {@link SecondaryStorage}.
 *
 * This is NOT a convenience cache. Three correctness-bearing things ride on it,
 * and each one fails in a different direction if the operations are not atomic:
 *
 * - `@better-auth/agent-auth`'s JTI replay cache (`jtiCacheStorage:
 *   "secondary-storage"`). A non-atomic `getAndDelete` lets two concurrent
 *   presentations of the same agent JWT both observe the JTI as unused, and the
 *   replay is ACCEPTED. That is a security control failing OPEN.
 * - Rate limiting (`rateLimit.storage: "secondary-storage"`), which calls
 *   `increment` and compares the result to the rule's max. A read-then-write
 *   increment loses counts under concurrency and silently raises the effective
 *   limit.
 * - Verification values, which better-auth consumes with `getAndDelete` so that
 *   a one-time code is redeemable exactly once.
 *
 * Both operations below are therefore expressed as a SINGLE statement using
 * SQLite's `RETURNING`, so the read and the write cannot be interleaved. Do not
 * "simplify" either into a SELECT followed by a DELETE/UPDATE.
 *
 * Workers KV cannot back this. It is eventually consistent and has no atomic
 * read-modify-write, so a replay inside the propagation window is accepted. D1
 * is strongly consistent, which is why it is the store here.
 */

const TABLE = "better_auth_secondary_storage";

/**
 * Timestamps are epoch milliseconds, matching the `timestamp_ms` mode the
 * Drizzle schema uses for every other temporal column. `now` is bound from JS
 * rather than read from SQLite's `unixepoch()`, which is second-granular unless
 * given the `subsecond` modifier — a silent 1000x unit mismatch otherwise.
 */
const nowMs = (): number => Date.now();

const liveValue = (row: { value?: unknown } | null): string | null => {
  const value = row?.value;
  return typeof value === "string" ? value : null;
};

export const d1SecondaryStorage = (db: D1Database): SecondaryStorage => ({
  get: async (key) =>
    liveValue(
      await db
        .prepare(`SELECT value FROM ${TABLE} WHERE key = ? AND (expires_at IS NULL OR expires_at > ?)`)
        .bind(key, nowMs())
        .first(),
    ),

  /**
   * Atomic: the row is deleted and its value returned in one statement, so a
   * concurrent caller cannot also observe it. This is what makes JTI replay
   * detection and one-time verification codes actually single-use.
   */
  getAndDelete: async (key) =>
    liveValue(
      await db
        .prepare(
          `DELETE FROM ${TABLE}
           WHERE key = ? AND (expires_at IS NULL OR expires_at > ?)
           RETURNING value`,
        )
        .bind(key, nowMs())
        .first(),
    ),

  set: async (key, value, ttl) => {
    await db
      .prepare(
        `INSERT INTO ${TABLE} (key, value, expires_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, expires_at = excluded.expires_at`,
      )
      .bind(key, value, ttl === undefined ? null : nowMs() + ttl * 1000)
      .run();
  },

  delete: async (key) => {
    await db.prepare(`DELETE FROM ${TABLE} WHERE key = ?`).bind(key).run();
  },

  /**
   * Atomic counter with the same expiry semantics as the PostgreSQL
   * implementation this replaces: an EXPIRED row starts a fresh window at 1
   * rather than resuming the old count, and a live row keeps its original
   * deadline so the window does not slide forward on every hit.
   *
   * Without the expired-row reset, a counter that ages out would resume from
   * its stale value and rate-limit a caller that should have been forgiven.
   */
  increment: async (key, ttl) => {
    const now = nowMs();
    const row = await db
      .prepare(
        `INSERT INTO ${TABLE} (key, value, expires_at) VALUES (?, '1', ?)
         ON CONFLICT(key) DO UPDATE SET
           value = CASE
             WHEN ${TABLE}.expires_at IS NOT NULL AND ${TABLE}.expires_at <= ?
               THEN '1'
             ELSE CAST(CAST(${TABLE}.value AS INTEGER) + 1 AS TEXT)
           END,
           expires_at = CASE
             WHEN ${TABLE}.expires_at IS NOT NULL AND ${TABLE}.expires_at <= ?
               THEN excluded.expires_at
             ELSE ${TABLE}.expires_at
           END
         RETURNING value`,
      )
      .bind(key, now + ttl * 1000, now, now)
      .first<{ value: string }>();

    const value = Number(row?.value);
    // Fail loudly. A silent 0 here reads as "under the limit" to the rate
    // limiter and as a usable counter to every other caller.
    if (!Number.isSafeInteger(value)) {
      throw new Error(`secondary-storage increment returned no usable value for ${key}`);
    }
    return value;
  },
});

/**
 * DDL for the table above. Applied at deploy time rather than per-request:
 * `CREATE TABLE IF NOT EXISTS` on every call would add a round trip to the hot
 * path of a security control.
 *
 * One statement per element — `D1Database.exec` refuses multi-statement strings.
 */
export const D1_SECONDARY_STORAGE_DDL = [
  `CREATE TABLE IF NOT EXISTS ${TABLE} (key TEXT PRIMARY KEY, value TEXT NOT NULL, expires_at INTEGER)`,
  `CREATE INDEX IF NOT EXISTS better_auth_secondary_storage_expires_at_idx ON ${TABLE} (expires_at) WHERE expires_at IS NOT NULL`,
] as const;
