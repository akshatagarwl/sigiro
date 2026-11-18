import { Context, Effect, Layer, Schema } from "effect";
import type { ClaimAssignment, CompleteJobRequest, WorkerClaimRequest } from "../src/contracts.ts";
import { ClaimAssignment as ClaimAssignmentSchema } from "../src/contracts.ts";
import { ExecutorError } from "./errors.ts";
import { completionFor, jobRequestOf, type JobRecord, type JobRequest } from "./types.ts";

export interface CoordinatorClientOptions {
  readonly coordinatorUrl: string;
  readonly workerId: string;
  readonly workerToken: string;
  readonly fetch?: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
  readonly sleep?: (milliseconds: number) => Promise<void>;
}

export class CoordinatorClient extends Context.Service<CoordinatorClient, {
  readonly claim: () => Effect.Effect<ClaimAssignment | undefined, ExecutorError>;
  readonly heartbeat: (request: JobRequest) => Effect.Effect<void, ExecutorError>;
  readonly complete: (record: JobRecord) => Effect.Effect<void, ExecutorError>;
}>()("sigiro/agents/CoordinatorClient") {}

const decodeAssignment = Schema.decodeUnknownEffect(ClaimAssignmentSchema);

export const makeCoordinatorClient = (options: CoordinatorClientOptions): Layer.Layer<CoordinatorClient> =>
  Layer.succeed(CoordinatorClient, CoordinatorClient.of({
    claim: () => request(options, "/v1/claim", {
      method: "POST",
      body: { workerId: options.workerId, claimId: crypto.randomUUID() } satisfies WorkerClaimRequest,
    }).pipe(Effect.flatMap((response) => response.status === 204
      ? Effect.succeed(undefined)
      : Effect.tryPromise({ try: async () => decodeAssignment(JSON.parse(await response.text())), catch: (cause) => new ExecutorError("decode claim", String(cause), cause) }).pipe(
        Effect.flatten,
        Effect.mapError((cause) => cause instanceof ExecutorError ? cause : new ExecutorError("decode claim", String(cause), cause)),
      ))),
    heartbeat: (job) => request(options, `/v1/jobs/${encodeURIComponent(job.jobId)}/heartbeat`, {
      method: "POST",
      body: { jobId: job.jobId, generation: job.generation, attempt: job.attempt, leaseToken: job.leaseToken },
    }).pipe(Effect.asVoid),
    complete: (record) => retryCompletion(options, completionFor(record)),
  }));

const transient = (status: number): boolean => status === 408 || status === 429 || status >= 500;
const retryDelay = (response: Response, attempt: number): number => {
  const value = Number(response.headers.get("retry-after"));
  return Number.isFinite(value) && value >= 0 ? value * 1_000 : 200 * 2 ** attempt;
};

const request = (options: CoordinatorClientOptions, path: string, init: { method: string; body: unknown }) =>
  Effect.tryPromise({
    try: async () => {
      const response = await (options.fetch ?? globalThis.fetch)(new URL(path, options.coordinatorUrl), {
        method: init.method,
        headers: { authorization: `Bearer ${options.workerToken}`, "content-type": "application/json" },
        body: JSON.stringify(init.body),
      });
      if (!response.ok && !(path === "/v1/claim" && response.status === 204)) {
        throw new ExecutorError("coordinator request", `HTTP ${response.status}`);
      }
      return response;
    },
    catch: (cause) => cause instanceof ExecutorError ? cause : new ExecutorError("coordinator request", String(cause), cause),
  });

const retryCompletion = (options: CoordinatorClientOptions, completion: CompleteJobRequest): Effect.Effect<void, ExecutorError> =>
  Effect.tryPromise({
    try: async () => {
      let last: unknown;
      for (let attempt = 0; attempt < 4; attempt += 1) {
        let response: Response;
        try {
          response = await (options.fetch ?? globalThis.fetch)(new URL(`/v1/jobs/${encodeURIComponent(completion.jobId)}/complete`, options.coordinatorUrl), {
            method: "POST",
            headers: { authorization: `Bearer ${options.workerToken}`, "content-type": "application/json" },
            body: JSON.stringify(completion),
          });
        } catch (cause) {
          last = cause;
          if (attempt === 3) throw cause;
          await (options.sleep ?? ((milliseconds: number) => Bun.sleep(milliseconds)))(200 * 2 ** attempt);
          continue;
        }
        if (response.ok || response.status === 409) return;
        const error = new ExecutorError("complete job", `HTTP ${response.status}`);
        if (!transient(response.status)) throw error;
        last = error;
        if (attempt < 3) await (options.sleep ?? ((milliseconds: number) => Bun.sleep(milliseconds)))(retryDelay(response, attempt));
      }
      throw last ?? new ExecutorError("complete job", "request failed");
    },
    catch: (cause) => cause instanceof ExecutorError ? cause : new ExecutorError("complete job", String(cause), cause),
  }).pipe(Effect.asVoid);

export { jobRequestOf };
