import type { D1Database } from "@cloudflare/workers-types";

/**
 * The three raw queries the auth service runs outside better-auth's own
 * adapter, ported from Effect/`pg` to D1.
 *
 * Identifiers stay double-quoted camelCase because better-auth creates the
 * columns that way; unquoted `activeorganizationid` is a different column as
 * far as SQLite is concerned and would return nothing.
 */

/**
 * The organization id on a session row, or null.
 *
 * Used to stamp the `tid` (tenant) claim. A wrong answer here hands a caller a
 * token scoped to someone else's tenant, so the failure modes are kept
 * distinguishable: "no such session" and "session with no active organization"
 * both return null, but neither is conflated with an error.
 */
export const sessionTenant = async (
  db: D1Database,
  sessionId: string
): Promise<string | null> => {
  const row = await db
    .prepare('SELECT "activeOrganizationId" FROM session WHERE id = ?')
    .bind(sessionId)
    .first<{ activeOrganizationId: string | null }>();
  return row?.activeOrganizationId ?? null;
};

/**
 * Every organization the user belongs to, oldest first.
 *
 * Callers depend on the ORDER BY for determinism: `introspectSigiroAgent`
 * rejects anything other than exactly one membership, and the ingest path reads
 * `[0]` as the tenant. An unordered result would make the tenant a race.
 */
export const membershipsForUser = async (
  db: D1Database,
  userId: string
): Promise<string[]> => {
  const { results } = await db
    .prepare(
      'SELECT "organizationId" FROM member WHERE "userId" = ? ORDER BY "createdAt", id'
    )
    .bind(userId)
    .all<{ organizationId: string }>();
  return results.map((row) => row.organizationId);
};

/**
 * The roles a user holds in one organization.
 *
 * better-auth stores multiple roles as a single COMMA-JOINED string, not as
 * rows — `"owner,admin"`. Splitting is therefore required, and an empty result
 * must yield `[]` rather than `[""]`, which would otherwise be a role name that
 * matches nothing but is not obviously wrong when read back.
 */
export const membershipRoles = async (
  db: D1Database,
  userId: string,
  organizationId: string
): Promise<string[]> => {
  const row = await db
    .prepare(
      'SELECT role FROM member WHERE "userId" = ? AND "organizationId" = ?'
    )
    .bind(userId, organizationId)
    .first<{ role: string }>();
  if (!row?.role) return [];
  return row.role
    .split(",")
    .map((role) => role.trim())
    .filter((role) => role.length > 0);
};

/** Liveness probe for `/health`. Throws if the binding cannot serve a query. */
export const checkDatabase = async (db: D1Database): Promise<void> => {
  await db.prepare("SELECT 1").first();
};
