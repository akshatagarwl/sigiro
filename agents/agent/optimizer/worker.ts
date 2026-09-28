import { createHmac } from "node:crypto";
import { Effect } from "effect";

import { TrustedGitHub, type Connection } from "./github.ts";
import { LeaseStore, type ClaimedRun, LeaseFailure } from "./leases.ts";

type Result = { pullRequestId: number };
export type RunExecutor = (
  run: ClaimedRun,
  signal: AbortSignal
) => Promise<Result>;

export class NeedsReconciliation extends Error {}

export function leaseBearer(run: ClaimedRun, secret: string): string {
  if (secret.length < 32) throw new Error("Eve signing secret is not configured");
  const now = Math.floor(Date.now() / 1000);
  const encode = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const body = `${encode({ alg: "HS256", typ: "JWT" })}.${encode({
    iss: "sigiro-optimizer", aud: "sigiro-eve", sub: run.id,
    tenant_id: run.tenant_id, lease_owner: run.leaseOwner, attempt: String(run.attempts),
    iat: now, exp: now + 50,
  })}`;
  return `${body}.${createHmac("sha256", secret).update(body).digest("base64url")}`;
}

export function withGitHubRecovery(
  store: LeaseStore,
  github: TrustedGitHub,
  perform: (
    run: ClaimedRun,
    target: { baseRef: string; targetCommitSha: string },
    signal: AbortSignal
  ) => Promise<Result>
): RunExecutor {
  return async (run, signal) => {
    const repositoryId = Number(run.repository_id);
    const installationId = Number(run.installation_id);
    if (
      !Number.isSafeInteger(repositoryId) ||
      !Number.isSafeInteger(installationId)
    ) {
      throw new NeedsReconciliation(
        "GitHub identifiers exceed JavaScript's safe integer range"
      );
    }
    const connection: Connection = {
      repositoryId,
      installationId,
      owner: run.owner,
      repo: run.repo,
    };
    let target: { baseRef: string; targetCommitSha: string };
    if (run.target_ref && run.target_commit_sha) {
      target = {
        baseRef: run.target_ref,
        targetCommitSha: run.target_commit_sha,
      };
    } else if (!run.target_ref && !run.target_commit_sha) {
      target = await Effect.runPromise(github.resolveTarget(connection));
      await Effect.runPromise(
        store.recordTarget(run, target.baseRef, target.targetCommitSha)
      );
    } else {
      throw new NeedsReconciliation(
        "Partial GitHub target cannot be recovered automatically"
      );
    }
    const existing = await Effect.runPromise(
      github.reconcile({ connection, runId: run.id, ...target })
    );
    if (existing) return { pullRequestId: existing.pullRequestId };
    if (run.attempts > 1 && !run.session_id) {
      throw new NeedsReconciliation(
        "Previous Rivet submission has no recorded session; do not create a duplicate turn"
      );
    }
    return perform(run, target, signal);
  };
}

export const runOnce = Effect.fn("optimizer.runOnce")(function* (
  store: LeaseStore,
  execute: RunExecutor,
  shutdown?: AbortSignal
) {
  const run = yield* store.claim();
  if (!run) return false;
  const controller = new AbortController();
  const onShutdown = () => controller.abort();
  if (shutdown?.aborted) controller.abort();
  else shutdown?.addEventListener("abort", onShutdown, { once: true });
  let leaseLost = false;
  const timer = setInterval(() => {
    void Effect.runPromise(store.renew(run)).catch(() => {
      leaseLost = true;
      controller.abort();
    });
  }, 20_000);
  const outcome = yield* Effect.gen(function* () {
    yield* store.ensureEnabled(run);
    const result = yield* Effect.tryPromise({
      try: () => execute(run, controller.signal),
      catch: (error) =>
        error instanceof NeedsReconciliation
          ? new LeaseFailure({
              message: `NEEDS_RECONCILIATION: ${error.message}`,
            })
          : new LeaseFailure({ message: String(error) }),
    });
    if (leaseLost)
      return yield* new LeaseFailure({
        message: "Lease lost during execution",
      });
    yield* store.ensureEnabled(run);
    yield* store.complete(run, result.pullRequestId);
    return true;
  }).pipe(
    Effect.result,
    Effect.ensuring(Effect.sync(() => {
      clearInterval(timer);
      shutdown?.removeEventListener("abort", onShutdown);
    }))
  );
  if (outcome._tag === "Success") return outcome.success;
  if (leaseLost || outcome.failure.message.includes("Lease lost"))
    return yield* outcome.failure;
  if (outcome.failure.message.startsWith("NEEDS_RECONCILIATION:")) {
    yield* store.park(run, outcome.failure.message);
  } else {
    yield* store.release(run);
  }
  return yield* outcome.failure;
});
