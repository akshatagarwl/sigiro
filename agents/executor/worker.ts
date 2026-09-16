import { DateTime, Effect, Fiber, Predicate } from "effect";
import { CoordinatorClient } from "./coordinator.ts";
import { EveExecution } from "./eve.ts";
import { JobStore } from "./services.ts";
import { jobRequestOf, type JobRecord } from "./types.ts";
import { sha256 } from "../src/signing.ts";

export interface RunOnceOptions { readonly client: CoordinatorClient["Service"]; readonly jobs: JobStore["Service"]; readonly eve: EveExecution["Service"]; }
const idFor = (request: { jobId: string; generation: number; attempt: number }) => `${request.jobId}:${request.generation}:${request.attempt}`;
const now = DateTime.now.pipe(Effect.map(DateTime.formatIso));
const failed = (record: JobRecord, cause: unknown, updatedAt: string): JobRecord => ({ ...record, status: "failed", error: cause instanceof Error ? cause.message : String(cause), updatedAt });
const reported = (record: JobRecord, timestamp: string): JobRecord => ({ ...record, reportedAt: timestamp, updatedAt: timestamp });
const report = Effect.fn("reportJob")(function* (options: RunOnceOptions, record: JobRecord) {
  const timestamp = yield* now;
  yield* options.client.complete(record);
  yield* options.jobs.put(reported(record, timestamp));
});

const execute = Effect.fn("executeJob")(function* (options: RunOnceOptions, initial: JobRecord) {
  const request = initial.request;
  const attributes = { "job.hash": yield* sha256(request.jobId), "execution.hash": yield* sha256(idFor(request)) };
  yield* Effect.annotateCurrentSpan(attributes);
  yield* Effect.annotateLogsScoped(attributes);
  yield* Effect.logInfo("job claimed");
  if (initial.status === "succeeded" || initial.status === "failed") {
    yield* report(options, initial); return;
  }
  let record = initial;
  yield* options.jobs.put({ ...record, status: "running", updatedAt: yield* now });
  const intervalMs = Math.max(1, Math.floor(request.deadlineSeconds * 1_000 / 3));
  const heartbeat = Effect.forever(Effect.sleep(`${intervalMs} millis`).pipe(Effect.andThen(options.client.heartbeat(request))));
  const heartbeatFiber = yield* Effect.forkScoped(heartbeat);
  const eve = options.eve;
  const active = options.client.heartbeat(request).pipe(Effect.andThen(eve.run(request)));
  const heartbeatFailure = Fiber.join(heartbeatFiber).pipe(Effect.flatMap(() => Effect.die(new Error("Celld heartbeat stopped"))));

  // A JS `try`/`catch` around `yield*` never sees an Effect failure: the failure
  // short-circuits the generator instead of throwing into it, so the old shape
  // left the record `running` forever and never reported it. Every branch below
  // is expressed with combinators so failures and interruption are both observed.
  const terminal = Effect.raceFirst(active, heartbeatFailure).pipe(
    Effect.matchEffect({
      onSuccess: (result) =>
        Effect.gen(function* () {
          const current = yield* options.jobs.get(idFor(request));
          if (Predicate.isUndefined(current)) return yield* Effect.die(new Error("local job result disappeared"));
          const succeeded: JobRecord = { ...current, status: "succeeded", receipt: result, updatedAt: yield* now };
          yield* options.jobs.put(succeeded);
          return succeeded;
        }),
      onFailure: (cause) =>
        Effect.gen(function* () {
          yield* Effect.logError("job execution failed", cause).pipe(Effect.annotateLogs({ "error.type": cause._tag }));
          // Typed operational failures retire the job. Defects and interruption
          // propagate, leaving the persisted session resumable.
          const current = yield* options.jobs.get(idFor(request));
          if (Predicate.isUndefined(current)) return yield* Effect.die(new Error("local job result disappeared"));
          let terminalRecord = failed(current, cause, yield* now);
          if (current.eveSessionId !== undefined) {
            yield* eve.cancel(current.eveSessionId).pipe(Effect.catch((cancellationError) =>
              Effect.gen(function* () {
                yield* Effect.logError("Eve cancellation failed", cancellationError);
                terminalRecord = { ...terminalRecord, error: `${terminalRecord.error}; cancellation failed: ${cancellationError.message}` };
              }),
            ));
          }
          if (current.eveSessionId !== undefined) {
            const reconciledAt = yield* now;
            yield* eve.receipt(request, current.eveSessionId).pipe(Effect.match({
              onSuccess: (receipt) => { if (receipt?.status === "published") terminalRecord = { ...current, status: "succeeded", receipt, updatedAt: reconciledAt }; },
              onFailure: (error) => { terminalRecord = { ...terminalRecord, error: `${terminalRecord.error}; receipt reconciliation failed: ${error.message}` }; },
            }));
          }
          yield* options.jobs.put(terminalRecord);
          return terminalRecord;
        }),
    }),
    Effect.ensuring(Fiber.interrupt(heartbeatFiber)),
  );

  record = yield* terminal;
  yield* report(options, record);
  yield* Effect.logInfo("job reported");
  return true;
}, Effect.scoped);

export const runOnce = Effect.fn("runOnce")(function* (options: RunOnceOptions) {
  const assignment = yield* options.client.claim();
  if (Predicate.isUndefined(assignment)) return false;
  const accepted = yield* options.jobs.create(jobRequestOf(assignment));
  yield* execute(options, accepted.record);
  return true;
});

export const drainUnreported = Effect.fn("drainUnreported")(function* (options: RunOnceOptions) {
  for (const record of yield* options.jobs.pending()) yield* execute(options, record);
});

// Each completed iteration is a root trace; a process-lifetime parent never exports.
export const workerProgram = Effect.fnUntraced(function* (pollSeconds: number) {
  const client = yield* Effect.service(CoordinatorClient); const jobs = yield* Effect.service(JobStore); const eve = yield* Effect.service(EveExecution);
  while (true) {
    yield* drainUnreported({ client, jobs, eve }).pipe(Effect.catch((error) => Effect.logError("pending report failed", error)));
    const processed = yield* runOnce({ client, jobs, eve }).pipe(Effect.catch((error) => Effect.logError("worker iteration failed", error).pipe(Effect.as(false))));
    if (!processed) yield* Effect.sleep(`${pollSeconds} seconds`);
  }
});
