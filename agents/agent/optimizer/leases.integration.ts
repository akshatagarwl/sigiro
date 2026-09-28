import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { setTimeout as sleep } from "node:timers/promises";

import { Effect } from "effect";
import { verifyJwtHmac } from "eve/channels/auth";
import { createEmulator } from "emulate";
import pg from "pg";

import { TrustedGitHub } from "./github.ts";
import { LeaseStore } from "./leases.ts";
import { OptimizerRuntime } from "./runtime.ts";
import { NeedsReconciliation, leaseBearer, runOnce, withGitHubRecovery } from "./worker.ts";

const name = `sigiro-optimizer-test-${randomUUID()}`;
execFileSync("docker", [
  "run",
  "-d",
  "--rm",
  "--name",
  name,
  "-e",
  "POSTGRES_PASSWORD=local-test-only",
  "-p",
  "127.0.0.1::5432",
  "postgres:18-alpine",
]);
const pool = new pg.Pool({
  host: "127.0.0.1",
  port: Number(
    execFileSync("docker", ["port", name, "5432/tcp"], { encoding: "utf8" })
      .trim()
      .split(":")
      .at(-1)
  ),
  user: "postgres",
  password: "local-test-only",
  database: "postgres",
  ssl: false,
});
try {
  for (let i = 0; i < 30; i++) {
    try {
      await pool.query("SELECT 1");
      break;
    } catch (error) {
      if (i === 29) throw error;
      await sleep(300);
    }
  }
  await pool.query(`CREATE SCHEMA sigiro_admin;
    CREATE TABLE sigiro_admin.optimizer_connection (
      tenant_id TEXT NOT NULL,id TEXT NOT NULL,installation_id BIGINT NOT NULL,repository_id BIGINT NOT NULL,
      owner TEXT NOT NULL,repo TEXT NOT NULL,enabled BOOLEAN NOT NULL,PRIMARY KEY(tenant_id,id));
    CREATE TABLE sigiro_admin.optimizer_run (
      tenant_id TEXT NOT NULL,id TEXT NOT NULL,service TEXT NOT NULL,connection_id TEXT NOT NULL,
      finding_id TEXT NOT NULL,branch TEXT NOT NULL,actor TEXT NOT NULL,status TEXT NOT NULL,
      lease_owner TEXT,lease_until_ms BIGINT,attempts INTEGER NOT NULL DEFAULT 0,
      session_id TEXT,failure_summary TEXT,github_pull_request_id BIGINT,
      target_ref TEXT,target_commit_sha TEXT,finding_before DOUBLE PRECISION,finding_after DOUBLE PRECISION,
      telemetry_window_start_us BIGINT,telemetry_window_end_us BIGINT,PRIMARY KEY(tenant_id,id));
    INSERT INTO sigiro_admin.optimizer_connection VALUES ('tenant-a','repo-a',100,123,'octocat','shop',TRUE),('tenant-b','repo-b',200,234,'octocat','other',FALSE);
    INSERT INTO sigiro_admin.optimizer_run (tenant_id,id,service,connection_id,finding_id,branch,actor,status) VALUES
      ('tenant-a','run-a','checkout','repo-a','finding-a','sigiro/optimizer-run-a','actor-a','pending'),
      ('tenant-b','run-b','checkout','repo-b','finding-b','sigiro/optimizer-run-b','actor-b','pending');`);
  const store = new LeaseStore(pool);
  const first = await Effect.runPromise(store.claim());
  assert.ok(first);
  assert.equal(first.tenant_id, "tenant-a");
  assert.equal(first.attempts, 1);
  const signingSecret = "local-test-key-not-for-production-long-enough";
  const credential = leaseBearer(first, signingSecret);
  const verified = await verifyJwtHmac(credential, { algorithm: "HS256", issuer: "sigiro-optimizer", audiences: ["sigiro-eve"], secret: signingSecret });
  assert.equal(verified.ok, true);
  if (verified.ok) {
    assert.equal(verified.sessionAuth.subject, first.id);
    assert.equal(verified.sessionAuth.attributes.lease_owner, first.leaseOwner);
    assert.equal(verified.sessionAuth.attributes.attempt, "1");
  }
  assert.equal((await verifyJwtHmac(`${credential}bad`, { algorithm: "HS256", issuer: "sigiro-optimizer", audiences: ["sigiro-eve"], secret: signingSecret })).ok, false);
  assert.equal(await Effect.runPromise(store.claim()), null);
  await Effect.runPromise(store.ensureEnabled(first));
  await Effect.runPromise(store.recordTarget(first, "main", "a".repeat(40)));
  await Effect.runPromise(store.recordSession(first, "eve-session-a"));
  await Effect.runPromise(store.renew(first));
  await pool.query(
    "UPDATE sigiro_admin.optimizer_run SET lease_until_ms=0 WHERE tenant_id='tenant-a'"
  );
  const second = await Effect.runPromise(store.claim());
  assert.ok(second);
  assert.equal(second.session_id, "eve-session-a");
  assert.equal(second.target_ref, "main");
  assert.equal(second.target_commit_sha, "a".repeat(40));
  await assert.rejects(
    Effect.runPromise(store.recordTarget(second, "changed", "b".repeat(40)))
  );
  assert.equal(second.attempts, 2);
  await assert.rejects(Effect.runPromise(store.complete(first, 999)));
  await assert.rejects(Effect.runPromise(store.complete(second, 0)));
  await Effect.runPromise(store.complete(second, 999));
  assert.equal(await Effect.runPromise(store.claim()), null);
  const result = await pool.query(
    "SELECT status,github_pull_request_id FROM sigiro_admin.optimizer_run WHERE tenant_id='tenant-a'"
  );
  assert.equal(result.rows[0].status, "completed");
  assert.equal(result.rows[0].github_pull_request_id, "999");
  await pool.query(`INSERT INTO sigiro_admin.optimizer_run
    (tenant_id,id,service,connection_id,finding_id,branch,actor,status)
    VALUES ('tenant-a','run-c','checkout','repo-a','finding-c','branch-c','actor-a','pending')`);
  for (let attempt = 1; attempt <= 3; attempt++) {
    const expired = await Effect.runPromise(store.claim());
    assert.ok(expired);
    assert.equal(expired.attempts, attempt);
    await pool.query(
      "UPDATE sigiro_admin.optimizer_run SET lease_until_ms=0 WHERE tenant_id='tenant-a' AND id='run-c'"
    );
  }
  assert.equal(await Effect.runPromise(store.claim()), null);
  assert.equal(
    (
      await pool.query(
        "SELECT status FROM sigiro_admin.optimizer_run WHERE id='run-c'"
      )
    ).rows[0].status,
    "needs_attention"
  );
  await pool.query(`INSERT INTO sigiro_admin.optimizer_run
    (tenant_id,id,service,connection_id,finding_id,branch,actor,status)
    VALUES ('tenant-a','run-d','checkout','repo-a','finding-d','branch-d','actor-a','pending')`);
  await assert.rejects(
    Effect.runPromise(
      runOnce(store, async () => {
        throw new NeedsReconciliation(
          "Eve accepted the turn but its session response was lost"
        );
      })
    )
  );
  assert.equal(
    (
      await pool.query(
        "SELECT status FROM sigiro_admin.optimizer_run WHERE id='run-d'"
      )
    ).rows[0].status,
    "needs_attention"
  );
  const github = await createEmulator({
    service: "github",
    port: 0,
    seed: {
      github: {
        users: [{ login: "octocat" }],
        repos: [{ owner: "octocat", name: "sample", auto_init: true }],
        apps: [
          {
            app_id: 12345,
            slug: "sigiro",
            name: "Sigiro",
            permissions: {
              contents: "write",
              pull_requests: "write",
              metadata: "read",
            },
            installations: [
              {
                installation_id: 100,
                account: "octocat",
                repository_selection: "all",
              },
            ],
          },
        ],
      },
    },
  });
  try {
    const key = github.generatedSecrets.find(
      (secret) =>
        secret.kind === "github.app_private_key" && secret.id === "12345"
    )?.value;
    assert.ok(key);
    const app = new TrustedGitHub({
      apiUrl: github.url,
      appId: "12345",
      privateKey: key,
    });
    const repoResponse = await fetch(`${github.url}/repos/octocat/sample`);
    const repo = (await repoResponse.json()) as { id: number };
    const connection = {
      repositoryId: repo.id,
      installationId: 100,
      owner: "octocat",
      repo: "sample",
    };
    const target = await Effect.runPromise(app.resolveTarget(connection));
    const id = randomUUID();
    const published = await Effect.runPromise(
      app.publish({
        connection,
        runId: id,
        ...target,
        changes: [{ path: "optimization.txt", content: "validated\n" }],
        title: "optimizer: validated",
        body: "Draft only",
      })
    );
    await pool.query(
      `INSERT INTO sigiro_admin.optimizer_connection VALUES ('tenant-a','repo-gh',100,$1,'octocat','sample',TRUE)`,
      [repo.id]
    );
    await pool.query(
      `INSERT INTO sigiro_admin.optimizer_run
      (tenant_id,id,service,connection_id,finding_id,branch,actor,status,target_ref,target_commit_sha)
      VALUES ('tenant-a',$1,'checkout','repo-gh','finding-gh',$2,'actor-a','pending',$3,$4)`,
      [id, published.branch, target.baseRef, target.targetCommitSha]
    );
    await Effect.runPromise(
      runOnce(
        store,
        withGitHubRecovery(store, app, async (claimed, resumedTarget) => {
          const result = await Effect.runPromise(
            app.publish({
              connection,
              runId: claimed.id,
              ...resumedTarget,
              changes: [{ path: "optimization.txt", content: "validated\n" }],
              title: "optimizer: validated",
              body: "Draft only",
            })
          );
          return { pullRequestId: result.pullRequestId };
        })
      )
    );
    const recovered = await pool.query(
      "SELECT status,github_pull_request_id FROM sigiro_admin.optimizer_run WHERE id=$1",
      [id]
    );
    assert.equal(recovered.rows[0].status, "completed");
    assert.equal(
      recovered.rows[0].github_pull_request_id,
      String(published.pullRequestId)
    );
    const newId = randomUUID();
    await pool.query(
      `INSERT INTO sigiro_admin.optimizer_run
      (tenant_id,id,service,connection_id,finding_id,branch,actor,status)
      VALUES ('tenant-a',$1,'checkout','repo-gh','finding-new',$2,'actor-a','pending')`,
      [newId, `sigiro/optimizer-${newId}`]
    );
    const publishedNew = await Effect.runPromise(
      runOnce(
        store,
        withGitHubRecovery(store, app, async (claimed, resolvedTarget) => {
          const result = await Effect.runPromise(
            app.publish({
              connection,
              runId: claimed.id,
              ...resolvedTarget,
              changes: [
                { path: "optimization-new.txt", content: "validated new\n" },
              ],
              title: "optimizer: validated new",
              body: "Draft only",
            })
          );
          return { pullRequestId: result.pullRequestId };
        })
      )
    );
    assert.equal(publishedNew, true);
    const created = await pool.query(
      "SELECT status,github_pull_request_id,target_ref,target_commit_sha FROM sigiro_admin.optimizer_run WHERE id=$1",
      [newId]
    );
    assert.equal(created.rows[0].status, "completed");
    assert.equal(created.rows[0].target_ref, target.baseRef);
    assert.equal(created.rows[0].target_commit_sha, target.targetCommitSha);
    assert.equal(
      (
        (await fetch(`${github.url}/repos/octocat/sample/pulls?state=all`).then(
          (r) => r.json()
        )) as unknown[]
      ).length,
      2
    );
    const guardedId = randomUUID();
    await pool.query(`INSERT INTO sigiro_admin.optimizer_run
      (tenant_id,id,service,connection_id,finding_id,branch,actor,status,
       finding_before,finding_after,telemetry_window_start_us,telemetry_window_end_us)
      VALUES ('tenant-a',$1,'checkout','repo-gh','finding-guarded',$2,'actor-a','pending',123,98,1779999000000000,1780001000000000)`,
      [guardedId, `sigiro/optimizer-${guardedId}`]);
    const guarded = await Effect.runPromise(store.claim());
    assert.ok(guarded);
    await Effect.runPromise(store.recordTarget(guarded, target.baseRef, target.targetCommitSha));
    await Effect.runPromise(store.recordSession(guarded, "eve-guarded"));
    const identity = await verifyJwtHmac(leaseBearer(guarded, signingSecret), {
      algorithm: "HS256", issuer: "sigiro-optimizer", audiences: ["sigiro-eve"], secret: signingSecret,
    });
    assert.equal(identity.ok, true);
    if (!identity.ok) throw new Error("Expected lease token");
    const runtime = new OptimizerRuntime(pool, app);
    const authorized = await runtime.forSession("eve-guarded", identity.sessionAuth);
    assert.equal(authorized.tenantId, "tenant-a");
    assert.equal(authorized.connection.repositoryId, repo.id);
    assert.equal(authorized.findingBefore, 123);
    assert.equal(authorized.windowStartUs, 1779999000000000);
    await assert.rejects(runtime.forSession("eve-guarded", {
      ...identity.sessionAuth, subject: "other-tenant-run",
    }));
    await pool.query("UPDATE sigiro_admin.optimizer_run SET lease_until_ms=0 WHERE id=$1", [guardedId]);
    await assert.rejects(runtime.forSession("eve-guarded", identity.sessionAuth));
  } finally {
    await github.close();
  }
  console.log(
    "PostgreSQL leases: duplicate claim, expiry recovery, fencing, disabled connection and PR completion passed"
  );
} finally {
  await pool.end();
  execFileSync("docker", ["rm", "-f", name]);
}
