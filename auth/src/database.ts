import * as PgConnection from "@effect/sql-pg/PgConnection";
import { Effect, Layer, ManagedRuntime, Schema } from "effect";

export type RunPg = <A, E>(effect: Effect.Effect<A, E, PgConnection.PgConnection>) => Promise<A>;

export const makePgRuntime = (config: PgConnection.Config) =>
  ManagedRuntime.make(
    Layer.effect(PgConnection.PgConnection, PgConnection.make(config)),
  );

export const query = Effect.fnUntraced(function* (
  statement: string,
  params: ReadonlyArray<unknown> = [],
) {
  const database = yield* PgConnection.PgConnection;
  return (yield* database.query(statement, params)).rows;
});

const SessionTenantRows = Schema.Array(Schema.Struct({ activeOrganizationId: Schema.NullOr(Schema.String) }));
const MembershipRows = Schema.Array(Schema.Struct({ organizationId: Schema.String }));
const RoleRows = Schema.Array(Schema.Struct({ role: Schema.String }));

export const sessionTenant = Effect.fn("Database.sessionTenant")(function* (sessionId: string) {
  const rows = yield* Schema.decodeUnknownEffect(SessionTenantRows)(yield* query(
    'SELECT "activeOrganizationId" FROM session WHERE id = $1',
    [sessionId],
  ));
  return rows[0]?.activeOrganizationId ?? null;
});

export const membershipsForUser = Effect.fn("Database.membershipsForUser")(function* (userId: string) {
  const rows = yield* Schema.decodeUnknownEffect(MembershipRows)(yield* query(
    'SELECT "organizationId" FROM member WHERE "userId" = $1 ORDER BY "createdAt", id',
    [userId],
  ));
  return rows.map((row) => row.organizationId);
});

export const membershipRoles = Effect.fn("Database.membershipRoles")(function* (
  userId: string,
  organizationId: string,
) {
  const rows = yield* Schema.decodeUnknownEffect(RoleRows)(yield* query(
    'SELECT role FROM member WHERE "userId" = $1 AND "organizationId" = $2',
    [userId, organizationId],
  ));
  return rows[0]?.role.split(",") ?? [];
});

export const checkDatabase = query("SELECT 1").pipe(Effect.asVoid);
