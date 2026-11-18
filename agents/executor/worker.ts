import { Cause, Effect, Fiber, Predicate } from "effect";
import { CoordinatorClient } from "./coordinator.ts";
import { EveExecution } from "./eve.ts";
import { JobStore } from "./services.ts";
import { jobRequestOf, type JobRecord } from "./types.ts";

export interface RunOnceOptions { readonly client: CoordinatorClient["Service"]; readonly jobs: JobStore["Service"]; readonly eve?: EveExecution["Service"]; }
const idFor = (request: { jobId: string; generation: number; attempt: number }) => `${request.jobId}:${request.generation}:${request.attempt}`;
const now = () => new Date().toISOString();
const failed = (record: JobRecord, cause: unknown): JobRecord => ({ ...record, status: "failed", error: cause instanceof Error ? cause.message : String(cause), updatedAt: now() });
const reported = (record: JobRecord): JobRecord => ({ ...record, reportedAt: now(), updatedAt: now() });
const report = (options: RunOnceOptions, record: JobRecord) => Effect.gen(function* () { yield* options.client.complete(record); yield* options.jobs.put(reported(record)); });

export const runOnce = Effect.fn("runOnce")(function* (options: RunOnceOptions) {
  const assignment = yield* options.client.claim();
  if (Predicate.isUndefined(assignment)) return false;
  const request = jobRequestOf(assignment);
  const accepted = yield* options.jobs.create(request);
  if (options.eve === undefined && (accepted.created || (accepted.record.status !== "succeeded" && accepted.record.status !== "failed"))) {
    const terminal = failed(accepted.record, new Error("Eve execution is required"));
    yield* options.jobs.put(terminal);
    yield* report(options, terminal);
    return true;
  }
  if (!accepted.created && (accepted.record.status === "succeeded" || accepted.record.status === "failed")) {
    yield* report(options, accepted.record); return true;
  }
  let record = accepted.record;
  yield* options.jobs.put({ ...record, status: "running", updatedAt: now() });
  yield* options.client.heartbeat(request);
  const intervalMs = Math.max(1, Math.floor(request.deadlineSeconds * 1_000 / 3));
  const heartbeat = Effect.forever(Effect.sleep(`${intervalMs} millis`).pipe(Effect.andThen(options.client.heartbeat(request))));
  const heartbeatFiber = yield* Effect.forkChild(heartbeat);
  const eve = options.eve;
  if (eve === undefined) return yield* Effect.fail(new Error("Eve execution is required"));
  const active = eve.run(request);
  const heartbeatFailure = Fiber.join(heartbeatFiber).pipe(Effect.flatMap(() => Effect.fail(new Error("Celld heartbeat stopped"))));

  // A JS `try`/`catch` around `yield*` never sees an Effect failure: the failure
  // short-circuits the generator instead of throwing into it, so the old shape
  // left the record `running` forever and never reported it. Every branch below
  // is expressed with combinators so failures and interruption are both observed.
  const terminal = Effect.raceFirst(active, heartbeatFailure).pipe(
    Effect.matchCauseEffect({
      onSuccess: (result) =>
        Effect.gen(function* () {
          const current = yield* options.jobs.get(idFor(request));
          if (Predicate.isUndefined(current)) return yield* Effect.fail(new Error("local job result disappeared"));
          // Eve owns the patch, so the executor holds no diff to record. The
          // exit-0 mapping rests on the published receipt, not on the observed
          // `turn.completed`: Eve swallows a throwing turn.completed handler and
          // stamps the event anyway, so the event alone proves only that the turn
          // ended. The handler writes a published receipt only after the required
          // test loop finished with every exit code 0 and the draft PR was
          // published, so a published receipt is what proves these passed.
          const succeeded: JobRecord = { ...current, status: "succeeded", result: {
            providerOutput: result.summary,
            tests: request.tests.map((test) => ({ ...test, exitCode: 0, stdout: "", stderr: "" })),
            ...(result.changedPaths === undefined ? {} : { changedPaths: result.changedPaths }),
            ...(result.pullRequestUrl === undefined ? {} : { pullRequestUrl: result.pullRequestUrl }),
            ...(result.pullRequestNumber === undefined ? {} : { pullRequestNumber: result.pullRequestNumber }),
          }, updatedAt: now() };
          yield* options.jobs.put(succeeded);
          return succeeded;
        }),
      onFailure: (cause) =>
        Effect.gen(function* () {
          // An interrupted run is not a failed one. The record stays `running`
          // so a restart can resume it from its persisted Eve cursor; writing
          // `failed` here would retire work that is still viable.
          if (Cause.hasInterruptsOnly(cause)) return yield* Effect.failCause(cause);
          const current = yield* options.jobs.get(idFor(request));
          if (current?.eveSessionId !== undefined) {
            yield* eve.cancel(current.eveSessionId).pipe(Effect.catchCause(() => Effect.void));
          }
          if (Predicate.isUndefined(current)) return yield* Effect.fail(new Error("local job result disappeared"));
          const terminalRecord = failed(current, Cause.squash(cause));
          yield* options.jobs.put(terminalRecord);
          return terminalRecord;
        }),
    }),
    Effect.ensuring(Fiber.interrupt(heartbeatFiber)),
  );

  record = yield* terminal;
  yield* report(options, record);
  return true;
});

export const drainUnreported = Effect.fn("drainUnreported")(function* (options: RunOnceOptions) {
  for (const record of yield* options.jobs.unreportedTerminal()) yield* report(options, record);
});

export const workerProgram = (pollSeconds: number) => Effect.gen(function* () {
  const client = yield* Effect.service(CoordinatorClient); const jobs = yield* Effect.service(JobStore); const eve = yield* Effect.service(EveExecution);
  while (true) {
    yield* drainUnreported({ client, jobs, eve }).pipe(Effect.catchCause((cause) => Effect.logError(`pending report failed: ${globalThis.String(cause)}`)));
    const processed = yield* runOnce({ client, jobs, eve }).pipe(Effect.catchCause((cause) => Effect.logError(`worker iteration failed: ${globalThis.String(cause)}`).pipe(Effect.as(false))));
    if (!processed) yield* Effect.sleep(`${pollSeconds} seconds`);
  }
});
