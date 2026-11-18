import { Client, ClientError } from "eve/client";
import { Context, DateTime, Effect, Layer, Option, Predicate, Schedule, Schema } from "effect";
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
  static from(detail: string, cause?: unknown): EveFailure {
    return new EveFailure(cause === undefined ? { detail } : { detail, cause });
  }
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
const asFailure = (cause: unknown): EveFailure => Schema.is(EveFailure)(cause) ? cause : EveFailure.from(cause instanceof Error && typeof cause.message === "string" ? cause.message : String(cause), cause);

export const makeEveExecution = (options: EveExecutionOptions): Layer.Layer<EveExecution, never, JobStore> =>
  Layer.effect(EveExecution, Effect.gen(function* () {
    const jobs = yield* JobStore;
    const client = new Client({ host: options.url, auth: { bearer: options.workerToken }, redirect: "error" });
    const dispatch = Effect.fn("EveExecution.dispatch")(function* (request: JobRequest) {
      const headers = yield* currentTraceHeaders;
      const body = yield* Schema.encodeEffect(Schema.fromJsonString(Schema.Unknown))(celldPayloadOf(request, options.workerId)).pipe(Effect.mapError(asFailure));
      return yield* Effect.tryPromise({
        try: (signal) => client.fetch("/internal/celld/job", {
          method: "POST", signal, headers: { ...headers, "content-type": "application/json" }, body,
        }), catch: asFailure,
      }).pipe(
        Effect.flatMap((response) => response.ok
          ? Effect.tryPromise({ try: () => response.json(), catch: asFailure }).pipe(
            Effect.flatMap(Schema.decodeUnknownEffect(Schema.Struct({ sessionId: Schema.NonEmptyString }))),
            Effect.mapError(asFailure),
            Effect.map(({ sessionId }) => sessionId),
          )
          : Effect.tryPromise({ try: () => response.text(), catch: asFailure }).pipe(
            Effect.flatMap((body) => Effect.fail(asFailure(new ClientError(response.status, body, response.headers)))),
          )),
      );
    });
    const sessionFor = Effect.fn("EveExecution.sessionFor")(function* (record: JobRecord) {
      if (record.eveSessionId !== undefined) return record.eveSessionId;
      const sessionId = yield* dispatch(record.request);
      yield* jobs.put({ ...record, eveSessionId: sessionId, updatedAt: yield* DateTime.now.pipe(Effect.map(DateTime.formatIso)) }).pipe(Effect.mapError(asFailure));
      return sessionId;
    });
    const readReceipt = Effect.fn("EveExecution.readReceipt")(function* (id: string) {
      const headers = yield* currentTraceHeaders;
      const body = yield* Schema.encodeEffect(Schema.fromJsonString(Schema.Unknown))( { executionId: id }).pipe(Effect.mapError(asFailure));
      return yield* Effect.tryPromise({
        try: (signal) => client.fetch("/internal/celld/job/result", {
          method: "POST", signal, headers: { ...headers, "content-type": "application/json" }, body,
        }), catch: asFailure,
      }).pipe(Effect.flatMap((response) => {
        if (response.status === 404) return Effect.succeed(undefined);
        return Effect.tryPromise({ try: () => response.ok ? response.json() : response.text(), catch: asFailure }).pipe(
          Effect.flatMap((value) => response.ok
            ? Schema.decodeUnknownEffect(PublicationReceipt)(value).pipe(Effect.mapError(asFailure))
            : Effect.fail(asFailure(new ClientError(response.status, String(value), response.headers)))),
          Effect.filterOrFail(
            (receipt) => receipt.executionId === id,
            () => EveFailure.from("Eve returned a receipt for another execution"),
          ),
        );
      }));
    });
    const receipt = Effect.fn("EveExecution.receipt")(function* (request: JobRequest, sessionId: string) {
      const result = yield* readReceipt(executionId(request)).pipe(Effect.timeout("5 seconds"), Effect.mapError(asFailure));
      if (result !== undefined && (result.sessionId !== sessionId || result.headSha !== request.baseSha)) return yield* EveFailure.from("Eve receipt identity does not match the job");
      return result;
    });
    const run = Effect.fn("EveExecution.run")(function* (request: JobRequest) {
      const id = executionId(request);
      const record = yield* jobs.get(id).pipe(Effect.mapError(asFailure));
      if (record === undefined) return yield* EveFailure.from("job disappeared");
      const acceptedAt = DateTime.make(record.acceptedAt);
      if (Option.isNone(acceptedAt)) return yield* EveFailure.from("invalid acceptedAt timestamp");
      const deadlineAt = DateTime.toEpochMillis(acceptedAt.value) + request.deadlineSeconds * 1000;
      const remaining = deadlineAt - DateTime.toEpochMillis(yield* DateTime.now);
      if (remaining <= 0) return yield* new EveDeadlineExceeded({ sessionId: record.eveSessionId ?? "not-dispatched" });
      const sessionId = yield* sessionFor(record).pipe(
        Effect.timeoutOrElse({ duration: `${remaining} millis`, orElse: () => Effect.fail(new EveDeadlineExceeded({ sessionId: record.eveSessionId ?? "not-dispatched" })) }),
      );
      // Eve owns execution durability. The trusted publisher's receipt is the outcome.
      const currentTime = yield* DateTime.now;
      const outcome = yield* receipt(request, sessionId).pipe(
        Effect.repeat({ schedule: Schedule.spaced("1 second"), until: Predicate.isNotUndefined<PublicationReceipt | undefined> }),
        Effect.timeoutOrElse({ duration: `${Math.max(1, deadlineAt - DateTime.toEpochMillis(currentTime))} millis`, orElse: () => Effect.fail(new EveDeadlineExceeded({ sessionId })) }),
      );
      if (outcome.status === "failed") return yield* EveFailure.from(outcome.error);
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
