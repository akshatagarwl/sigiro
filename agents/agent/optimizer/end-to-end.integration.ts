import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtemp, rm, rmdir } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { createEmulator, type Emulator } from "emulate";
import { Effect } from "effect";
import { cleanupCloudflareSandbox, cloudflareSandbox } from "./cloudflare.ts";
import pg from "pg";
import { TrustedGitHub } from "./github.ts";
import { LeaseStore } from "./leases.ts";
import { collectChanges, provisionWorkspace } from "./workspace.ts";
import { runOnce, withGitHubRecovery } from "./worker.ts";

if (!process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL || !process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY) {
  throw new Error("Cloudflare sandbox bridge URL and key are required for the composed integration");
}
const container = `sigiro-optimizer-flow-${randomUUID()}`;
execFileSync("docker", ["run", "-d", "--rm", "--name", container, "-e", "POSTGRES_PASSWORD=local-test-only", "-p", "127.0.0.1::5432", "postgres:18-alpine"]);
const port = Number(execFileSync("docker", ["port", container, "5432/tcp"], { encoding: "utf8" }).trim().split(":").at(-1));
const pool = new pg.Pool({ host: "127.0.0.1", port, user: "postgres", password: "local-test-only", database: "postgres", ssl: false });
let github: Emulator | undefined;
let sandbox: Awaited<ReturnType<ReturnType<typeof cloudflareSandbox>["create"]>> | undefined;
const originalSnapshotDirectory = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR;
const originalEncryptionKey = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
const snapshotDirectory = await mkdtemp(path.join(os.tmpdir(), "sigiro-cloudflare-flow-"));
process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR = snapshotDirectory;
process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = randomBytes(32).toString("hex");
const sessionId = randomUUID();
const sessionKey = `eve-sbx-ses-${sessionId}-root`;
const sandboxInput = { templateKey: null, sessionKey, tags: { sessionId, agent: "optimizer", channel: "eve" }, runtimeContext: { appRoot: process.cwd() } };
const backend = cloudflareSandbox();
try {
  for (let attempt = 0; attempt < 30; attempt++) {
    try { await pool.query("SELECT 1"); break; }
    catch (error) { if (attempt === 29) throw error; await sleep(300); }
  }
  await pool.query(`CREATE SCHEMA sigiro_admin;
    CREATE TABLE sigiro_admin.optimizer_connection (tenant_id TEXT NOT NULL,id TEXT NOT NULL,installation_id BIGINT NOT NULL,
      repository_id BIGINT NOT NULL,owner TEXT NOT NULL,repo TEXT NOT NULL,enabled BOOLEAN NOT NULL,PRIMARY KEY(tenant_id,id));
    CREATE TABLE sigiro_admin.optimizer_run (tenant_id TEXT NOT NULL,id TEXT NOT NULL,service TEXT NOT NULL,connection_id TEXT NOT NULL,
      finding_id TEXT NOT NULL,branch TEXT NOT NULL,actor TEXT NOT NULL,status TEXT NOT NULL,
      lease_owner TEXT,lease_until_ms BIGINT,attempts INTEGER NOT NULL DEFAULT 0,session_id TEXT,
      failure_summary TEXT,github_pull_request_id BIGINT,target_ref TEXT,target_commit_sha TEXT,PRIMARY KEY(tenant_id,id));`);
  const emulator = await createEmulator({ service: "github", port: 0, seed: { github: {
    users: [{ login: "octocat" }], repos: [{ owner: "octocat", name: "sample", auto_init: true }],
    apps: [{ app_id: 12345, slug: "sigiro", name: "Sigiro", permissions: { contents: "write", pull_requests: "write", metadata: "read" },
      installations: [{ installation_id: 100, account: "octocat", repository_selection: "all" }] }],
  } } });
  github = emulator;
  const key = emulator.generatedSecrets.find((secret) => secret.kind === "github.app_private_key" && secret.id === "12345")?.value;
  assert.ok(key);
  const app = new TrustedGitHub({ apiUrl: emulator.url, appId: "12345", privateKey: key });
  const repoResponse = await fetch(`${emulator.url}/repos/octocat/sample`);
  assert.equal(repoResponse.status, 200);
  const repo = await repoResponse.json() as { id: number };
  await pool.query("INSERT INTO sigiro_admin.optimizer_connection VALUES ('tenant-a','repo-a',100,$1,'octocat','sample',TRUE)", [repo.id]);
  const id = randomUUID();
  await pool.query(`INSERT INTO sigiro_admin.optimizer_run
    (tenant_id,id,service,connection_id,finding_id,branch,actor,status)
    VALUES ('tenant-a',$1,'checkout','repo-a','finding-a',$2,'actor-a','pending')`, [id, `sigiro/optimizer-${id}`]);
  sandbox = await backend.create(sandboxInput);
  let io = sandbox.session;
  const store = new LeaseStore(pool);
  const finished = await Effect.runPromise(runOnce(store, withGitHubRecovery(store, app, async (run, target) => {
    const files = await Effect.runPromise(app.checkout({ owner: run.owner, repo: run.repo,
      repositoryId: Number(run.repository_id), installationId: Number(run.installation_id) }, target.targetCommitSha));
    await provisionWorkspace(io, files);
    const edited = await io.run({ command: "printf 'optimized checkout\\n' > /workspace/repo/README.md" });
    assert.equal(edited.exitCode, 0);
    const validation = await io.run({ command: "cd /workspace/repo && grep -q 'not present' README.md" });
    assert.notEqual(validation.exitCode, 0);
    const checkpoint = await sandbox!.captureState();
    await sandbox!.shutdown();
    sandbox = await backend.create({ ...sandboxInput, existingMetadata: checkpoint.metadata });
    io = sandbox.session;
    const changes = await collectChanges(io);
    const published = await Effect.runPromise(app.publish({ connection: { owner: run.owner, repo: run.repo,
      repositoryId: Number(run.repository_id), installationId: Number(run.installation_id) },
      runId: run.id, ...target, changes, title: "optimizer: telemetry-backed change",
      body: "Validation: FAILED (exit 1). Draft only; human review required." }));
    return { pullRequestId: published.pullRequestId };
  })));
  assert.equal(finished, true);
  const saved = await pool.query("SELECT status,github_pull_request_id,target_ref,target_commit_sha FROM sigiro_admin.optimizer_run WHERE id=$1", [id]);
  assert.equal(saved.rows[0].status, "completed");
  assert.ok(saved.rows[0].github_pull_request_id);
  assert.ok(saved.rows[0].target_commit_sha);
  const pullsResponse = await fetch(`${emulator.url}/repos/octocat/sample/pulls?state=all`);
  const pulls = await pullsResponse.json() as Array<{ draft: boolean; body: string }>;
  assert.equal(pulls.length, 1);
  assert.equal(pulls[0].draft, true);
  assert.match(pulls[0].body, /FAILED/);
  await cleanupCloudflareSandbox(sessionId);
  sandbox = undefined;
  console.log("PostgreSQL lease → GitHub App checkout → Cloudflare sandbox edit/restore/failed validation → draft PR → persisted result passed");
} finally {
  if (sandbox) await sandbox.shutdown();
  if (github) await github.close();
  await pool.end();
  execFileSync("docker", ["rm", "-f", container]);
  const digest = createHash("sha256").update(sessionId).digest("hex");
  await rm(path.join(snapshotDirectory, `${digest}.tar`), { force: true });
  await rm(path.join(snapshotDirectory, `${digest}.json`), { force: true });
  await rmdir(snapshotDirectory);
  if (originalSnapshotDirectory === undefined) delete process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR;
  else process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR = originalSnapshotDirectory;
  if (originalEncryptionKey === undefined) delete process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
  else process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = originalEncryptionKey;
}
