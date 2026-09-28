import { setTimeout as sleep } from "node:timers/promises";
import { Client } from "eve/client";
import { Effect } from "effect";
import { LeaseStore, type ClaimedRun } from "./leases.ts";
import { cleanupCloudflareSandbox, CloudflareBridge } from "./cloudflare.ts";
import { optimizerRuntime } from "./runtime.ts";
import { NeedsReconciliation, leaseBearer, runOnce, withGitHubRecovery } from "./worker.ts";

const runtime = optimizerRuntime();
const store = new LeaseStore(runtime.pool);
const signingSecret = process.env.SIGIRO_OPTIMIZER_EVE_HMAC_SECRET;
const eveUrl = process.env.SIGIRO_OPTIMIZER_EVE_URL;
if (!signingSecret || signingSecret.length < 32 || !eveUrl) throw new Error("Optimizer Eve connection is not configured");
const sandboxUrl = process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL;
const sandboxKey = process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY;
if (!sandboxUrl || !sandboxKey || !process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR)
  throw new Error("Optimizer Cloudflare sandbox is not configured");
new CloudflareBridge(sandboxUrl, sandboxKey);
const url = new URL(eveUrl);
if (url.protocol !== "https:" && !(url.protocol === "http:" && ["127.0.0.1", "localhost", "::1"].includes(url.hostname))) {
  throw new Error("Eve connection must use TLS or loopback");
}
const shutdown = new AbortController();
process.once("SIGTERM", () => shutdown.abort());
process.once("SIGINT", () => shutdown.abort());

async function execute(run: ClaimedRun, target: { baseRef: string; targetCommitSha: string }, signal: AbortSignal) {
  const client = new Client({
    host: eveUrl!, redirect: "error",
    auth: { bearer: () => leaseBearer(run, signingSecret!) },
  });
  let sessionId = run.session_id;
  if (sessionId) {
    const session = client.session({ sessionId, streamIndex: 0 });
    for await (const event of session.stream({ startIndex: 0, signal })) {
      if (["turn.completed", "turn.failed", "turn.cancelled", "session.completed", "session.failed"].includes(event.type)) break;
    }
  } else {
    const session = client.session();
    let response;
    try {
      response = await session.send({
        message: `Assess fresh tenant telemetry for ${run.service} before changing anything. Assessor decision context: ${run.decision_context}. Query the live evidence window first; abstain if the data is inadequate. Only then prepare /workspace/repo, make one focused change, validate and call submit_optimizer_draft.`,
        signal,
      });
    } catch {
      throw new NeedsReconciliation("Eve session creation had no confirmed response; manual session reconciliation required");
    }
    await Effect.runPromise(store.recordSession(run, response.sessionId));
    sessionId = response.sessionId;
    await response.result();
  }
  const repositoryId = Number(run.repository_id);
  const installationId = Number(run.installation_id);
  const existing = await Effect.runPromise(runtime.github.reconcile({
    connection: { repositoryId, installationId, owner: run.owner, repo: run.repo },
    runId: run.id, ...target,
  }));
  if (!existing) throw new NeedsReconciliation("Rivet turn ended without a confirmed draft PR");
  if (sessionId) {
    try { await cleanupCloudflareSandbox(sessionId); }
    catch { console.error("Optimizer sandbox cleanup requires operator attention"); }
  }
  return { pullRequestId: existing.pullRequestId };
}

const recover = withGitHubRecovery(store, runtime.github, execute);
const perform = async (run: ClaimedRun, signal: AbortSignal) => {
  const result = await recover(run, signal);
  if (run.session_id) {
    try { await cleanupCloudflareSandbox(run.session_id); }
    catch { console.error("Recovered optimizer sandbox cleanup requires operator attention"); }
  }
  return result;
};
try {
  while (!shutdown.signal.aborted) {
    try {
      const claimed = await Effect.runPromise(runOnce(store, perform, shutdown.signal));
      if (!claimed) await sleep(2000, undefined, { signal: shutdown.signal });
    } catch (error) {
      if (shutdown.signal.aborted) break;
      console.error("Optimizer worker run failed:", error instanceof Error ? error.name : "unknown");
      await sleep(2000, undefined, { signal: shutdown.signal });
    }
  }
} finally {
  await runtime.pool.end();
}
