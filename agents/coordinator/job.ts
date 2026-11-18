import { DateTime, Duration, Effect, Schema, Semaphore } from "effect";
import {
  ClaimAssignment,
  CompleteJobRequest,
  CreateJobRequest,
  JobClaim,
  WorkerClaimRequest,
  LeaseRef,
  PublicStoredJob,
  StoredJob,
  executionIdOf,
  isTerminalJobStatus,
} from "../src/contracts.ts";
import { sha256 } from "../src/signing.ts";
import { HttpServerRequest } from "effect/unstable/http";
import { BoundedJson, json } from "./http.ts";
import type { CoordinatorEnv, DurableObjectState } from "./runtime.ts";

const JOB_KEY = "job";
const decodeCreateJob = Schema.decodeUnknownEffect(BoundedJson(CreateJobRequest));
const decodeJobClaim = Schema.decodeUnknownEffect(BoundedJson(WorkerClaimRequest));
const decodeLease = Schema.decodeUnknownEffect(BoundedJson(LeaseRef));
const decodeComplete = Schema.decodeUnknownEffect(BoundedJson(CompleteJobRequest));
const decodeStoredJob = Schema.decodeUnknownEffect(StoredJob);

export class StorageFailure extends Schema.TaggedError<StorageFailure>()("StorageFailure", {
  operation: Schema.String,
  cause: Schema.Defect(),
}) {
  override get message(): string { return `failed to ${this.operation}`; }
}

export class QueueUnavailable extends Schema.TaggedError<QueueUnavailable>()("QueueUnavailable", {
  cause: Schema.Defect(),
}) {
  override get message(): string { return "job queue unavailable"; }
}

export class Job {
  readonly #state: DurableObjectState;
  readonly #env: CoordinatorEnv;
  readonly #claims = Semaphore.makeUnsafe(1);

  constructor(state: DurableObjectState, env: CoordinatorEnv) {
    this.#state = state;
    this.#env = env;
  }

  fetch(request: Request): Promise<Response> {
    const route = `${request.method} ${new URL(request.url).pathname}`;
    const effect = route === "POST /claim" ? this.#claims.withPermit(this.#claim(request))
      : route === "POST /create" ? this.#create(request)
      : route === "GET /status" ? this.#status()
      : route === "POST /heartbeat" ? this.#heartbeat(request)
      : route === "POST /complete" ? this.#complete(request)
      : Effect.succeed(json({ error: "not found" }, 404));
    return Effect.runPromise(Effect.match(effect.pipe(Effect.tapCause(Effect.logError)), {
      onFailure: (error) => Schema.isSchemaError(error)
        ? json({ error: error.message }, 400)
        : json({ error: error.message }, 500),
      onSuccess: (response) => response,
    }));
  }

  alarm(): Promise<void> {
    return Effect.runPromise(this.#expireLease());
  }

  readonly #load = Effect.fn("Job.load")({ self: this }, function*(this: Job) {
    const value = yield* Effect.tryPromise({
      try: () => this.#state.storage.get<unknown>(JOB_KEY),
      catch: (cause) => new StorageFailure({ operation: "load job", cause }),
    });
    return value === undefined ? undefined : yield* decodeStoredJob(value).pipe(
      Effect.mapError((cause) => new StorageFailure({ operation: "validate stored job", cause })),
    );
  });

  readonly #store = Effect.fn("Job.store")({ self: this }, function*(this: Job, job: StoredJob) {
    yield* Effect.tryPromise({
      try: () => this.#state.storage.put(JOB_KEY, job),
      catch: (cause) => new StorageFailure({ operation: "persist job", cause }),
    });
    return job;
  });

  readonly #setAlarm = Effect.fn("Job.setAlarm")({ self: this }, function*(this: Job, at: number) {
    yield* Effect.tryPromise({
      try: () => this.#state.storage.setAlarm(at),
      catch: (cause) => new StorageFailure({ operation: "schedule lease", cause }),
    });
  });

  readonly #enqueue = Effect.fn("Job.enqueue")({ self: this }, function*(this: Job, jobId: string) {
    const response = yield* Effect.tryPromise({
      try: () => this.#env.QUEUE.get(this.#env.QUEUE.idFromName("global")).fetch(
        new Request("http://queue.internal/enqueue", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ jobId }),
        }),
      ),
      catch: (cause) => new QueueUnavailable({ cause }),
    });
    if (!response.ok) return yield* new QueueUnavailable({ cause: new Error(`queue returned ${response.status}`) });
  });

  readonly #create = Effect.fn("Job.create")({ self: this }, function*(this: Job, request: Request) {
    const spec = yield* decodeCreateJob(yield* HttpServerRequest.fromWeb(request).text);
    const specSha256 = yield* sha256(JSON.stringify(spec));
    const existing = yield* this.#load();
    if (existing !== undefined) {
      if (existing.specSha256 !== specSha256) return json({ error: "job id already exists with different immutable input" }, 409);
      if (existing.status === "queued") yield* this.#enqueue(existing.spec.jobId);
      return json(publicJob(existing));
    }
    const now = DateTime.formatIso(yield* DateTime.now);
    const job: StoredJob = { spec, specSha256, status: "queued", attempt: 0, createdAt: now, updatedAt: now };
    yield* this.#store(job);
    yield* this.#enqueue(spec.jobId);
    return json(publicJob(job), 201);
  });

  readonly #status = Effect.fn("Job.status")({ self: this }, function*(this: Job) {
    const job = yield* this.#load();
    return job === undefined ? json({ error: "job not found" }, 404) : json(publicJob(job));
  });

  readonly #claim = Effect.fn("Job.claim")({ self: this }, function*(this: Job, request: Request) {
    const { workerId, claimId } = yield* decodeJobClaim(yield* HttpServerRequest.fromWeb(request).text);
    const job = yield* this.#load();
    if (job === undefined) return json({ error: "job not found" }, 404);
    if (job.status === "accepted" && job.claim?.claimId === claimId) {
      const now = yield* DateTime.now;
      const leaseExpiresAt = Date.parse(job.claim.leaseExpiresAt) <= DateTime.toEpochMillis(now)
        ? DateTime.formatIso(DateTime.addDuration(now, Duration.seconds(job.spec.policy.deadlineSeconds)))
        : job.claim.leaseExpiresAt;
      if (leaseExpiresAt !== job.claim.leaseExpiresAt) {
        yield* this.#store({ ...job, claim: { ...job.claim, leaseExpiresAt }, updatedAt: DateTime.formatIso(now) });
      }
      yield* this.#setAlarm(Date.parse(leaseExpiresAt));
      return json({
        job: job.spec, attempt: job.attempt,
        executionId: executionIdOf(job.spec.jobId, job.spec.generation, job.attempt),
        leaseToken: job.claim.leaseToken, leaseExpiresAt,
      } satisfies ClaimAssignment);
    }
    if (job.status !== "queued") return json({ error: "job is not queued" }, 409);
    const now = yield* DateTime.now;
    const attempt = job.attempt + 1;
    if (attempt > job.spec.policy.maxAttempts) {
      const failed: StoredJob = { ...job, status: "failed", lastError: "maximum attempts exhausted", updatedAt: DateTime.formatIso(now) };
      yield* this.#store(failed);
      return json({ error: "job is not claimable" }, 409);
    }
    const leaseExpiresAt = DateTime.formatIso(DateTime.addDuration(now, Duration.seconds(job.spec.policy.deadlineSeconds)));
    const claim: JobClaim = { workerId, claimId, leaseToken: crypto.randomUUID(), leaseExpiresAt };
    yield* this.#store({ ...job, status: "accepted", attempt, claim, updatedAt: DateTime.formatIso(now) });
    yield* this.#setAlarm(DateTime.toEpochMillis(DateTime.addDuration(now, Duration.seconds(job.spec.policy.deadlineSeconds))));
    const assignment: ClaimAssignment = {
      job: job.spec, attempt, executionId: executionIdOf(job.spec.jobId, job.spec.generation, attempt),
      leaseToken: claim.leaseToken, leaseExpiresAt,
    };
    return json(assignment);
  });

  readonly #heartbeat = Effect.fn("Job.heartbeat")({ self: this }, function*(this: Job, request: Request) {
    const ref = yield* decodeLease(yield* HttpServerRequest.fromWeb(request).text);
    const job = yield* this.#load();
    if (job === undefined) return json({ error: "job not found" }, 404);
    if (!isCurrentLease(job, ref)) return json({ error: "stale job generation, attempt, or lease" }, 409);
    const now = yield* DateTime.now;
    if (isLeaseExpired(job, now)) return json({ error: "lease has expired" }, 409);
    if (job.status !== "accepted" && job.status !== "running") return json({ error: `cannot heartbeat ${job.status} job` }, 409);
    const leaseExpiresAt = DateTime.formatIso(DateTime.addDuration(now, Duration.seconds(job.spec.policy.deadlineSeconds)));
    const updated: StoredJob = { ...job, status: "running", claim: { ...job.claim!, leaseExpiresAt }, updatedAt: DateTime.formatIso(now) };
    yield* this.#store(updated);
    yield* this.#setAlarm(DateTime.toEpochMillis(DateTime.addDuration(now, Duration.seconds(job.spec.policy.deadlineSeconds))));
    return json(publicJob(updated));
  });

  readonly #complete = Effect.fn("Job.complete")({ self: this }, function*(this: Job, request: Request) {
    const completion = yield* decodeComplete(yield* HttpServerRequest.fromWeb(request).text);
    const job = yield* this.#load();
    if (job === undefined) return json({ error: "job not found" }, 404);
    if (!isCurrentLease(job, completion)) return json({ error: "stale job generation, attempt, or lease" }, 409);
    const now = yield* DateTime.now;
    if (isLeaseExpired(job, now)) return json({ error: "lease has expired" }, 409);
    if (isTerminalJobStatus(job.status)) {
      if (job.status === completion.outcome && JSON.stringify(job.result) === JSON.stringify(completion.result)) return json(publicJob(job));
      return json({ error: `job is already ${job.status}` }, 409);
    }
    const completed: StoredJob = { ...job, status: completion.outcome, result: completion.result, updatedAt: DateTime.formatIso(yield* DateTime.now) };
    yield* this.#store(completed);
    yield* Effect.tryPromise({ try: () => this.#state.storage.deleteAlarm(), catch: (cause) => new StorageFailure({ operation: "clear lease", cause }) });
    return json(publicJob(completed));
  });

  readonly #expireLease = Effect.fn("Job.expireLease")({ self: this }, function*(this: Job) {
    const job = yield* this.#load();
    if (job === undefined) return;
    if (job.status === "queued") {
      yield* this.#enqueue(job.spec.jobId);
      return;
    }
    if (job.status !== "accepted" && job.status !== "running") return;
    const now = yield* DateTime.now;
    if (job.claim === undefined || Date.parse(job.claim.leaseExpiresAt) > DateTime.toEpochMillis(now)) {
      if (job.claim !== undefined) yield* this.#setAlarm(Date.parse(job.claim.leaseExpiresAt));
      return;
    }
    if (job.attempt >= job.spec.policy.maxAttempts) {
      const { claim: _claim, ...withoutClaim } = job;
      yield* this.#store({ ...withoutClaim, status: "failed", lastError: "lease expired after maximum attempts", updatedAt: DateTime.formatIso(now) });
      return;
    }
    const { claim: _claim, ...withoutClaim } = job;
    const queued: StoredJob = { ...withoutClaim, status: "queued", updatedAt: DateTime.formatIso(now) };
    yield* this.#store(queued);
    yield* this.#enqueue(job.spec.jobId);
  }, (effect) => effect.pipe(
    Effect.tapErrorTag("QueueUnavailable", (error) => Effect.logError(error).pipe(
      Effect.andThen(DateTime.now),
      Effect.flatMap((now) => this.#setAlarm(DateTime.toEpochMillis(now) + 1_000).pipe(
        Effect.mapError((alarmFailure) => new StorageFailure({
          operation: "reschedule queued job after queue failure",
          cause: new AggregateError([error, alarmFailure], "queue enqueue and retry scheduling both failed"),
        })),
      )),
    )),
  ));
}

const isCurrentLease = (job: StoredJob, ref: LeaseRef | CompleteJobRequest): boolean =>
  job.claim !== undefined && ref.jobId === job.spec.jobId && ref.generation === job.spec.generation &&
  ref.attempt === job.attempt && ref.leaseToken === job.claim.leaseToken;

const isLeaseExpired = (job: StoredJob, now: Parameters<typeof DateTime.toEpochMillis>[0]): boolean =>
  job.claim !== undefined && Date.parse(job.claim.leaseExpiresAt) <= DateTime.toEpochMillis(now);

const publicJob = (job: StoredJob): PublicStoredJob => ({
  spec: job.spec, specSha256: job.specSha256, status: job.status, attempt: job.attempt,
  createdAt: job.createdAt, updatedAt: job.updatedAt,
  ...(job.claim === undefined ? {} : { workerId: job.claim.workerId, leaseExpiresAt: job.claim.leaseExpiresAt }),
  ...(job.result === undefined ? {} : { result: job.result }),
  ...(job.lastError === undefined ? {} : { lastError: job.lastError }),
});
