import { readFileSync } from "node:fs";
import pg from "pg";
import { TrustedGitHub, type Connection } from "./github.ts";

export type AuthorizedRun = {
  id: string;
  tenantId: string;
  service: string;
  findingId: string;
  findingBefore: number | null;
  findingAfter: number | null;
  windowStartUs: number | null;
  windowEndUs: number | null;
  leaseOwner: string;
  attempts: number;
  branch: string;
  baseRef: string;
  targetCommitSha: string;
  connection: Connection;
};

export type LeaseIdentity = {
  readonly authenticator: string;
  readonly principalType: string;
  readonly subject?: string;
  readonly attributes: Readonly<Record<string, string | readonly string[]>>;
};

export class OptimizerRuntime {
  readonly pool: pg.Pool;
  readonly github: TrustedGitHub;
  constructor(pool: pg.Pool, github: TrustedGitHub) {
    this.pool = pool;
    this.github = github;
  }

  async telemetry(run: AuthorizedRun, sql: string) {
    const token = process.env.SIGIRO_OPTIMIZER_INTERNAL_TOKEN;
    const apiUrl = process.env.SIGIRO_INTERNAL_API_URL;
    if (!token || token.length < 32 || !apiUrl) throw new Error("Tenant telemetry bridge is not configured");
    const response = await fetch(new URL("/internal/optimizer/query", apiUrl), {
      method: "POST", redirect: "error", signal: AbortSignal.timeout(15_000),
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ run_id: run.id, lease_owner: run.leaseOwner, attempts: run.attempts, sql }),
    });
    if (!response.ok) throw new Error(`Tenant telemetry query rejected (${response.status})`);
    return response.json() as Promise<{ rows: unknown[]; truncated: boolean }>;
  }

  async forSession(sessionId: string, auth: LeaseIdentity | null): Promise<AuthorizedRun> {
    if (!auth || auth.authenticator !== "jwt-hmac" || auth.principalType !== "service") {
      throw new Error("Optimizer session is not authenticated");
    }
    const result = await this.pool.query(`SELECT r.id,r.tenant_id,r.service,r.finding_id,r.branch,r.target_ref,r.target_commit_sha,
      r.lease_owner,r.attempts,r.finding_before,r.finding_after,r.telemetry_window_start_us,
      r.telemetry_window_end_us,c.installation_id::text AS installation_id,c.repository_id::text AS repository_id,c.owner,c.repo
      FROM sigiro_admin.optimizer_run r JOIN sigiro_admin.optimizer_connection c
        ON c.tenant_id=r.tenant_id AND c.id=r.connection_id
      WHERE r.session_id=$1 AND r.status='running' AND c.enabled=TRUE
        AND r.lease_until_ms >= (extract(epoch FROM clock_timestamp()) * 1000)::bigint`, [sessionId]);
    if (result.rowCount !== 1) throw new Error("No active optimizer lease for this session");
    const row = result.rows[0] as Record<string, unknown>;
    if (auth.subject !== row.id || auth.attributes.tenant_id !== row.tenant_id ||
      auth.attributes.lease_owner !== row.lease_owner || auth.attributes.attempt !== String(row.attempts)) {
      throw new Error("Optimizer caller does not own the active lease");
    }
    if (typeof row.target_ref !== "string" || typeof row.target_commit_sha !== "string") {
      throw new Error("Optimizer run has no pinned GitHub target");
    }
    const repositoryId = Number(row.repository_id);
    const installationId = Number(row.installation_id);
    if (!Number.isSafeInteger(repositoryId) || !Number.isSafeInteger(installationId)) {
      throw new Error("GitHub identifiers exceed JavaScript's safe integer range");
    }
    return {
      id: row.id as string, tenantId: row.tenant_id as string, service: row.service as string,
      findingId: row.finding_id as string, leaseOwner: row.lease_owner as string,
      findingBefore: row.finding_before as number | null, findingAfter: row.finding_after as number | null,
      windowStartUs: row.telemetry_window_start_us === null ? null : Number(row.telemetry_window_start_us),
      windowEndUs: row.telemetry_window_end_us === null ? null : Number(row.telemetry_window_end_us),
      attempts: row.attempts as number, branch: row.branch as string,
      baseRef: row.target_ref, targetCommitSha: row.target_commit_sha,
      connection: { repositoryId, installationId, owner: row.owner as string, repo: row.repo as string },
    };
  }
}

let instance: OptimizerRuntime | undefined;
export function optimizerRuntime(): OptimizerRuntime {
  if (instance) return instance;
  const databaseUrl = process.env.SIGIRO_OPTIMIZER_DATABASE_URL;
  const caFile = process.env.SIGIRO_OPTIMIZER_PG_CA_FILE;
  const appId = process.env.SIGIRO_GITHUB_APP_ID;
  const privateKey = process.env.SIGIRO_GITHUB_APP_PRIVATE_KEY;
  if (!databaseUrl || !caFile || !appId || !privateKey) throw new Error("Optimizer host credentials are not configured");
  const url = new URL(databaseUrl);
  if (!url.hostname || !["postgres:", "postgresql:"].includes(url.protocol) || url.searchParams.get("sslmode") !== "verify-full") {
    throw new Error("Optimizer PostgreSQL requires a verified TLS connection");
  }
  url.searchParams.delete("sslmode");
  url.searchParams.delete("sslrootcert");
  const pool = new pg.Pool({ connectionString: url.toString(), ssl: { ca: readFileSync(caFile), rejectUnauthorized: true, servername: url.hostname }, max: 5 });
  instance = new OptimizerRuntime(pool, new TrustedGitHub({ apiUrl: process.env.SIGIRO_GITHUB_API_URL ?? "https://api.github.com", appId, privateKey }));
  return instance;
}
