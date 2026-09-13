import { Client, ClientError } from "eve/client";
import { Context, Effect, Layer, Predicate, Schedule, Schema } from "effect";
import { JobStore } from "./services.ts";
import type { JobRecord, JobRequest } from "./types.ts";
import { celldPayloadOf, executionId } from "./types.ts";
import { PublicationReceipt } from "../src/publication.ts";
import { currentTraceHeaders } from "./telemetry.ts";

export class EveDeadlineExceeded extends Schema.TaggedError<EveDeadlineExceeded>()("EveDeadlineExceeded", {
  sessionId: Schema.String,
}) {
  override get message(): string { return `Eve deadline exceeded for session ${this.sessionId}`; }
}

export class EveFailure extends Schema.TaggedError<EveFailure>()("EveFailure", {
  detail: Schema.String,
  cause: Schema.optionalKey(Schema.Defect()),
}) {
  constructor(detail: string, cause?: unknown) { super(cause === undefined ? { detail } : { detail, cause }); }
  override get message(): string { return this.detail; }
}

export type EveError = EveDeadlineExceeded | EveFailure;
export type EveResult = Extract<PublicationReceipt, { readonly status: "published" }>;
export class EveExecution extends Context.Service<EveExecution, {
  readonly run: (request: JobRequest) => Effect.Effect<EveResult, EveError>;
  readonly receipt: (request: JobRequest, sessionId: string) => Effect.Effect<PublicationReceipt | undefined, EveError>;
  readonly cancel: (sessionId: string) => Effect.Effect<void, EveError>;
}>()("sigiro/agents/EveExecution") {}

export interface EveExecutionOptions { readonly url: string; readonly workerToken: string; readonly workerId: string; }
const asFailure = (cause: unknown): EveFailure => cause instanceof EveFailure ? cause : new EveFailure(cause instanceof Error && typeof cause.message === "string" ? cause.message : String(cause), cause);
const remainingDeadlineMillis = (acceptedAt: string, deadlineSeconds: number): number =>
  Date.parse(acceptedAt) + deadlineSeconds * 1000 - Date.now();

export const makeEveExecution = (options: EveExecutionOptions): Layer.Layer<EveExecution, never, JobStore> =>
  Layer.effect(EveExecution, Effect.gen(function* () {
    const jobs = yield* JobStore;
    const client = new Client({ host: options.url, auth: { bearer: options.workerToken }, redirect: "error" });
    const dispatch = Effect.fn("EveExecution.dispatch")(function* (request: JobRequest) {
      const headers = yield* currentTraceHeaders;
      return yield* Effect.tryPromise({
        try: async (signal) => {
          const response = await client.fetch("/internal/celld/job", {
            method: "POST", signal, headers: { ...headers, "content-type": "application/json" },
            body: JSON.stringify(celldPayloadOf(request, options.workerId)),
          });
          if (!response.ok) throw new ClientError(response.status, await response.text(), response.headers);
          return Schema.decodeUnknownSync(Schema.Struct({ sessionId: Schema.NonEmptyString }))(await response.json()).sessionId;
        }, catch: asFailure,
      });
    });
    const sessionFor = Effect.fn("EveExecution.sessionFor")(function* (record: JobRecord) {
      if (record.eveSessionId !== undefined) return record.eveSessionId;
      const sessionId = yield* dispatch(record.request);
      yield* jobs.put({ ...record, eveSessionId: sessionId, updatedAt: new Date().toISOString() }).pipe(Effect.mapError(asFailure));
      return sessionId;
    });
    const readReceipt = Effect.fn("EveExecution.readReceipt")(function* (id: string) {
      const headers = yield* currentTraceHeaders;
      return yield* Effect.tryPromise({
        try: async (signal) => {
          const response = await client.fetch("/internal/celld/job/result", {
            method: "POST", signal, headers: { ...headers, "content-type": "application/json" }, body: JSON.stringify({ executionId: id }),
          });
          if (response.status === 404) return undefined;
          if (!response.ok) throw new ClientError(response.status, await response.text(), response.headers);
          const receipt = Schema.decodeUnknownSync(PublicationReceipt)(await response.json());
          if (receipt.executionId !== id) throw new EveFailure("Eve returned a receipt for another execution");
          return receipt;
        }, catch: asFailure,
      });
    });
    const receipt = Effect.fn("EveExecution.receipt")(function* (request: JobRequest, sessionId: string) {
      const result = yield* readReceipt(executionId(request)).pipe(Effect.timeout("5 seconds"), Effect.mapError(asFailure));
      if (result !== undefined && (result.sessionId !== sessionId || result.headSha !== request.baseSha)) return yield* new EveFailure("Eve receipt identity does not match the job");
      return result;
    });
    const run = Effect.fn("EveExecution.run")(function* (request: JobRequest) {
      const id = executionId(request);
      const record = yield* jobs.get(id).pipe(Effect.mapError(asFailure));
      if (record === undefined) return yield* new EveFailure("job disappeared");
      const remaining = remainingDeadlineMillis(record.acceptedAt, request.deadlineSeconds);
      if (remaining <= 0) return yield* new EveDeadlineExceeded({ sessionId: record.eveSessionId ?? "not-dispatched" });
      const sessionId = yield* sessionFor(record).pipe(
        Effect.timeout(`${remaining} millis`),
        Effect.catchTag("TimeoutError", () => new EveDeadlineExceeded({ sessionId: record.eveSessionId ?? "not-dispatched" })),
      );
      // Eve owns execution durability. The trusted publisher's receipt is the outcome.
      const outcome = yield* receipt(request, sessionId).pipe(
        Effect.repeat({ schedule: Schedule.spaced("1 second"), until: Predicate.isNotUndefined<PublicationReceipt | undefined> }),
        Effect.timeout(`${Math.max(1, remainingDeadlineMillis(record.acceptedAt, request.deadlineSeconds))} millis`),
        Effect.catchTag("TimeoutError", () => new EveDeadlineExceeded({ sessionId })),
      );
      if (outcome.status === "failed") return yield* new EveFailure(outcome.error);
      return outcome;
    });
    // The worker owns cancellation, including heartbeat failures and deadlines.
    const cancel = Effect.fn("EveExecution.cancel")(function* (sessionId: string) {
      const session = client.sessions.attach(sessionId);
      const result = yield* Effect.tryPromise({ try: () => session.cancel(), catch: asFailure }).pipe(Effect.timeout("5 seconds"), Effect.mapError(asFailure));
      if (result.status === "no_active_turn") return;
      yield* Effect.tryPromise({
        try: async (signal) => {
          for await (const event of session.stream({ follow: true, signal })) {
            if (event.type === "turn.cancelled") return;
          }
          throw new Error("Eve cancellation was not confirmed");
        }, catch: asFailure,
      }).pipe(Effect.timeout("5 seconds"), Effect.mapError(asFailure));
    });
    return EveExecution.of({ run, cancel, receipt });
  }));
