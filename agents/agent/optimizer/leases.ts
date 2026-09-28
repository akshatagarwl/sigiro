import { randomUUID } from "node:crypto";

import { Effect, Schema } from "effect";
import type { Pool } from "pg";

export class LeaseFailure extends Schema.TaggedError<LeaseFailure>()(
  "LeaseFailure",
  {
    message: Schema.String,
  }
) {}

const Claimed = Schema.Struct({
  tenant_id: Schema.String,
  id: Schema.String,
  service: Schema.String,
  connection_id: Schema.String,
  finding_id: Schema.String,
  branch: Schema.String,
  actor: Schema.String,
  attempts: Schema.Number,
  session_id: Schema.NullOr(Schema.String),
  target_ref: Schema.NullOr(Schema.String),
  target_commit_sha: Schema.NullOr(Schema.String),
  installation_id: Schema.String,
  repository_id: Schema.String,
  owner: Schema.String,
  repo: Schema.String,
});

export type ClaimedRun = Schema.Schema.Type<typeof Claimed> & {
  leaseOwner: string;
};

export class LeaseStore {
  private readonly pool: Pool;
  constructor(pool: Pool) {
    this.pool = pool;
  }

  claim() {
    return Effect.tryPromise({
      try: async (): Promise<ClaimedRun | null> => {
        const client = await this.pool.connect();
        try {
          await client.query("BEGIN");
          await client.query(`UPDATE sigiro_admin.optimizer_run SET status='needs_attention',
            lease_owner=NULL,lease_until_ms=NULL,failure_summary='Retry limit reached after lease expiry'
            WHERE status='running' AND attempts>=3
            AND lease_until_ms < (extract(epoch FROM clock_timestamp()) * 1000)::bigint`);
          const result = await client.query(`
            SELECT r.tenant_id,r.id,r.service,r.connection_id,r.finding_id,r.branch,r.actor,r.attempts,r.session_id,r.target_ref,r.target_commit_sha,
                   c.installation_id::text AS installation_id,c.repository_id::text AS repository_id,c.owner,c.repo
            FROM sigiro_admin.optimizer_run r
            JOIN sigiro_admin.optimizer_connection c ON c.tenant_id=r.tenant_id AND c.id=r.connection_id
            WHERE c.enabled=TRUE AND (r.status='pending' OR
              (r.status='running' AND r.lease_until_ms < (extract(epoch FROM clock_timestamp()) * 1000)::bigint))
              AND r.attempts < 3
            ORDER BY r.tenant_id,r.id LIMIT 1 FOR UPDATE OF r SKIP LOCKED`);
          const record: unknown = result.rows[0];
          if (!record) {
            await client.query("COMMIT");
            return null;
          }
          const row = Schema.decodeUnknownSync(Claimed)(record);
          const leaseOwner = randomUUID();
          await client.query(
            `UPDATE sigiro_admin.optimizer_run SET status='running',lease_owner=$3,
            lease_until_ms=(extract(epoch FROM clock_timestamp()) * 1000)::bigint+60000,
            attempts=attempts+1 WHERE tenant_id=$1 AND id=$2`,
            [row.tenant_id, row.id, leaseOwner]
          );
          await client.query("COMMIT");
          return { ...row, leaseOwner, attempts: row.attempts + 1 };
        } catch (error) {
          await client.query("ROLLBACK");
          throw error;
        } finally {
          client.release();
        }
      },
      catch: (error) => new LeaseFailure({ message: String(error) }),
    });
  }

  private update(
    run: ClaimedRun,
    sql: string,
    values: readonly unknown[] = []
  ) {
    return Effect.tryPromise({
      try: async () => {
        const result = await this.pool.query(sql, [
          run.tenant_id,
          run.id,
          run.leaseOwner,
          run.attempts,
          ...values,
        ]);
        if (result.rowCount !== 1)
          throw new Error("Lease lost; refusing stale worker write");
      },
      catch: (error) => new LeaseFailure({ message: String(error) }),
    });
  }

  renew(run: ClaimedRun) {
    return this.update(
      run,
      `UPDATE sigiro_admin.optimizer_run SET
      lease_until_ms=(extract(epoch FROM clock_timestamp()) * 1000)::bigint+60000
      WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4 AND status='running'
      AND lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`
    );
  }

  recordTarget(run: ClaimedRun, baseRef: string, sha: string) {
    if (!/^[a-zA-Z0-9._/-]+$/.test(baseRef) || !/^[a-f0-9]{40}$/.test(sha)) {
      return Effect.fail(
        new LeaseFailure({ message: "Invalid GitHub target" })
      );
    }
    return this.update(
      run,
      `UPDATE sigiro_admin.optimizer_run SET target_ref=$5,target_commit_sha=$6
      WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4 AND status='running'
      AND target_ref IS NULL AND target_commit_sha IS NULL
      AND lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`,
      [baseRef, sha]
    );
  }

  recordSession(run: ClaimedRun, sessionId: string) {
    return this.update(
      run,
      `UPDATE sigiro_admin.optimizer_run SET session_id=$5
      WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4 AND status='running'
      AND session_id IS NULL AND lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`,
      [sessionId]
    );
  }

  complete(run: ClaimedRun, pullRequestId: number) {
    if (!Number.isSafeInteger(pullRequestId) || pullRequestId <= 0) {
      return Effect.fail(
        new LeaseFailure({ message: "A confirmed GitHub PR ID is required" })
      );
    }
    return this.update(
      run,
      `UPDATE sigiro_admin.optimizer_run SET status='completed',github_pull_request_id=$5,
      lease_owner=NULL,lease_until_ms=NULL WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4
      AND status='running' AND lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`,
      [pullRequestId]
    );
  }

  release(run: ClaimedRun) {
    return this.update(
      run,
      `UPDATE sigiro_admin.optimizer_run SET status=CASE WHEN attempts >= 3 THEN 'needs_attention' ELSE 'pending' END,
      lease_owner=NULL,lease_until_ms=NULL WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4
      AND status='running' AND lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`
    );
  }

  ensureEnabled(run: ClaimedRun) {
    return Effect.tryPromise({
      try: async () => {
        const result = await this.pool.query(
          `SELECT 1 FROM sigiro_admin.optimizer_connection c
          JOIN sigiro_admin.optimizer_run r ON r.tenant_id=c.tenant_id AND r.connection_id=c.id
          WHERE r.tenant_id=$1 AND r.id=$2 AND r.lease_owner=$3 AND r.attempts=$4
          AND r.status='running' AND c.enabled=TRUE
          AND r.lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`,
          [run.tenant_id, run.id, run.leaseOwner, run.attempts]
        );
        if (result.rowCount !== 1)
          throw new Error("Lease lost or repository connection disabled");
      },
      catch: (error) => new LeaseFailure({ message: String(error) }),
    });
  }

  park(run: ClaimedRun, reason: string) {
    return this.update(
      run,
      `UPDATE sigiro_admin.optimizer_run SET status='needs_attention',failure_summary=$5,
      lease_owner=NULL,lease_until_ms=NULL WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4
      AND status='running' AND lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`,
      [reason.slice(0, 512)]
    );
  }
}
