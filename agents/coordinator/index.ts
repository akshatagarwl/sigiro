import { Effect, Schema, Semaphore } from "effect";
import { HttpServerRequest } from "effect/unstable/http";
import { Job } from "./job.ts";
import { BoundedJson, json } from "./http.ts";
import { ClaimId, JobIdentity } from "../src/contracts.ts";
import type { CoordinatorEnv, DurableObjectState, DurableObjectStub } from "./runtime.ts";

export { Job };
const decodeIdentity = Schema.decodeUnknownEffect(BoundedJson(JobIdentity));
const PublicWorkerClaimRequest = Schema.Struct({ workerId: Schema.NonEmptyString });
const decodeWorker = Schema.decodeUnknownEffect(BoundedJson(PublicWorkerClaimRequest));
const QUEUE_KEY = "pending";

const PendingEntry = Schema.Struct({ jobId: Schema.NonEmptyString, claimId: ClaimId });
const Pending = Schema.Array(PendingEntry);
type Pending = typeof Pending.Type;

class QueueStorageFailure extends Schema.TaggedError<QueueStorageFailure>()("QueueStorageFailure", {
  operation: Schema.String,
  cause: Schema.Defect(),
}) {
  override get message(): string { return `failed to ${this.operation}`; }
}

class CoordinatorUnavailable extends Schema.TaggedError<CoordinatorUnavailable>()("CoordinatorUnavailable", {
  operation: Schema.String,
  cause: Schema.Defect(),
}) {
  override get message(): string { return `failed to ${this.operation}`; }
}

const forward = Effect.fn("forward")(function*(stub: DurableObjectStub, request: Request) {
  return yield* Effect.tryPromise({
    try: () => stub.fetch(request),
    catch: (cause) => new CoordinatorUnavailable({ operation: `${request.method} ${new URL(request.url).pathname}`, cause }),
  });
});

const unavailable = Effect.fn("unavailable")(function*(error: CoordinatorUnavailable) {
  yield* Effect.logError(error);
  return Response.json({ error: "coordinator unavailable" }, { status: 503, headers: { "retry-after": "1", "cache-control": "no-store" } });
});

/** Compare bearer credentials without leaking a byte-by-byte timing oracle. */
const tokenMatches = (supplied: string, expected: string): boolean => {
  const a = new TextEncoder().encode(supplied);
  const b = new TextEncoder().encode(expected);
  let difference = a.length ^ b.length;
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    difference |= (a[index] ?? 0) ^ (b[index] ?? 0);
  }
  return difference === 0;
};

const authorized = (request: Request, env: CoordinatorEnv): boolean => {
  const authorization = request.headers.get("authorization");
  return authorization !== null && tokenMatches(authorization, `Bearer ${env.WORKER_TOKEN}`);
};

export class JobQueue {
  readonly #state: DurableObjectState;
  readonly #env: CoordinatorEnv;
  readonly #mutations = Semaphore.makeUnsafe(1);

  constructor(state: DurableObjectState, env: CoordinatorEnv) {
    this.#state = state;
    this.#env = env;
  }

  fetch(request: Request): Promise<Response> {
    const route = `${request.method} ${new URL(request.url).pathname}`;
    const effect = route === "POST /claim" ? this.#claim(request)
      : route === "POST /enqueue" ? this.#enqueue(request)
      : Effect.succeed(json({ error: "not found" }, 404));
    return Effect.runPromise(Effect.match(this.#mutations.withPermit(effect).pipe(Effect.tapCause(Effect.logError)), {
      onFailure: (error) => Schema.isSchemaError(error) ? json({ error: String(error) }, 400) : json({ error: String(error) }, 500),
      onSuccess: (response) => response,
    }));
  }

  readonly #load = Effect.fn("JobQueue.load")({ self: this }, function*(this: JobQueue) {
    const value = yield* Effect.tryPromise({
      try: () => this.#state.storage.get<unknown>(QUEUE_KEY),
      catch: (cause) => new QueueStorageFailure({ operation: "load queue", cause }),
    });
    if (value === undefined) return [];
    return yield* Schema.decodeUnknownEffect(Pending)(value).pipe(
      Effect.mapError((cause) => new QueueStorageFailure({ operation: "validate queue", cause })),
    );
  });

  readonly #save = Effect.fn("JobQueue.save")({ self: this }, function*(this: JobQueue, pending: Pending) {
    yield* Effect.tryPromise({
      try: () => this.#state.storage.put(QUEUE_KEY, pending),
      catch: (cause) => new QueueStorageFailure({ operation: "persist queue", cause }),
    });
  });

  readonly #enqueue = Effect.fn("JobQueue.enqueue")({ self: this }, function*(this: JobQueue, request: Request) {
    const { jobId } = yield* decodeIdentity(yield* HttpServerRequest.fromWeb(request).text);
    const pending = yield* this.#load();
    if (!pending.some((entry) => entry.jobId === jobId)) {
      yield* this.#save([...pending, { jobId, claimId: crypto.randomUUID() }]);
    }
    return json({ queued: true });
  });

  readonly #claim = Effect.fn("JobQueue.claim")({ self: this }, function*(this: JobQueue, request: Request) {
    const { workerId } = yield* decodeWorker(yield* HttpServerRequest.fromWeb(request).text);
    let pending = yield* this.#load();
    for (let index = 0; index < pending.length; index += 1) {
      const entry = pending[index]!;
      const stub = this.#env.JOBS.get(this.#env.JOBS.idFromName(entry.jobId));
      const response = yield* forward(stub, new Request("http://job.internal/claim", {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ workerId, claimId: entry.claimId }),
      }));
      if (response.status === 200) {
        yield* this.#save(pending.filter((_, candidate) => candidate !== index));
        return response;
      }
      if (response.status === 404 || response.status === 409) {
        pending = pending.filter((_, candidate) => candidate !== index);
        yield* this.#save(pending);
        index -= 1;
        continue;
      }
      return yield* new CoordinatorUnavailable({ operation: "claim job", cause: new Error(`job returned ${response.status}`) });
    }
    return new Response(null, { status: 204, headers: { "retry-after": "1" } });
  }, Effect.catchTag("CoordinatorUnavailable", unavailable));
}

const handle = Effect.fn("handle")(function*(request: Request, env: CoordinatorEnv) {
  const url = new URL(request.url);
  if (request.method === "POST" && url.pathname === "/v1/claim") {
    if (!authorized(request, env)) return json({ error: "unauthorized" }, 401);
    const body = yield* HttpServerRequest.fromWeb(request).text;
    yield* decodeWorker(body);
    return yield* forward(env.QUEUE.get(env.QUEUE.idFromName("global")), new Request("http://queue.internal/claim", {
      method: "POST", headers: { "content-type": "application/json" }, body,
    }));
  }
  if (request.method === "POST" && url.pathname === "/v1/jobs") {
    if (!authorized(request, env)) return json({ error: "unauthorized" }, 401);
    const body = yield* HttpServerRequest.fromWeb(request).text;
    const routing = yield* decodeIdentity(body);
    return yield* forward(env.JOBS.get(env.JOBS.idFromName(routing.jobId)), new Request("http://job.internal/create", {
      method: "POST", headers: request.headers, body,
    }));
  }
  const match = /^\/v1\/jobs\/([^/]+)(?:\/(heartbeat|complete))?$/.exec(url.pathname);
  if (match === null) return json({ error: "not found" }, 404);
  const jobId = decodeURIComponent(match[1]!);
  const action = match[2];
  const stub: DurableObjectStub = env.JOBS.get(env.JOBS.idFromName(jobId));
  if (request.method === "GET" && action === undefined) {
    if (!authorized(request, env)) return json({ error: "unauthorized" }, 401);
    return yield* forward(stub, new Request("http://job.internal/status"));
  }
  if (request.method !== "POST" || action === undefined) return json({ error: "not found" }, 404);
  if ((action === "heartbeat" || action === "complete") && !authorized(request, env)) {
    return json({ error: "unauthorized" }, 401);
  }
  const body = yield* HttpServerRequest.fromWeb(request).text;
  return yield* forward(stub, new Request(`http://job.internal/${action}`, {
    method: "POST", headers: request.headers, body,
  }));
}, Effect.catchTag("CoordinatorUnavailable", unavailable));

export default {
  fetch(request: Request, env: CoordinatorEnv): Promise<Response> {
    return Effect.runPromise(Effect.match(handle(request, env).pipe(Effect.tapCause(Effect.logError)), {
      onFailure: (error) => Schema.isSchemaError(error) ? json({ error: error.message }, 400) : json({ error: error.message }, 500),
      onSuccess: (response) => response,
    }));
  },
};
