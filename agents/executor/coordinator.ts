import { BunCrypto } from "@effect/platform-bun";
import { Context, Crypto, Effect, Layer, Schedule } from "effect";
import { FetchHttpClient, HttpClient, HttpClientRequest, HttpClientResponse } from "effect/unstable/http";
import { ClaimAssignment, type WorkerClaimRequest } from "../src/contracts.ts";
import { ExecutorError } from "./errors.ts";
import { completionFor, jobRequestOf, type JobRecord, type JobRequest } from "./types.ts";

export interface CoordinatorClientOptions {
  readonly coordinatorUrl: string;
  readonly workerId: string;
  readonly workerToken: string;
}

export class CoordinatorClient extends Context.Service<CoordinatorClient, {
  readonly claim: () => Effect.Effect<ClaimAssignment | undefined, ExecutorError>;
  readonly heartbeat: (request: JobRequest) => Effect.Effect<void, ExecutorError>;
  readonly complete: (record: JobRecord) => Effect.Effect<void, ExecutorError>;
}>()("sigiro/agents/CoordinatorClient") {}

export const makeCoordinatorClient = (options: CoordinatorClientOptions): Layer.Layer<CoordinatorClient> =>
  Layer.effect(CoordinatorClient, Effect.gen(function* () {
    const client = (yield* HttpClient.HttpClient).pipe(HttpClient.mapRequest((request) => request.pipe(
      HttpClientRequest.prependUrl(options.coordinatorUrl),
      HttpClientRequest.bearerToken(options.workerToken),
    )));
    const ordinary = client.pipe(HttpClient.filterStatusOk);

    // Replaying a completion is safe: celld accepts it or retires its lease (409).
    const completion = client.pipe(
      HttpClient.filterStatus((status) => (status >= 200 && status < 300) || status === 409),
      HttpClient.retryTransient({ schedule: Schedule.exponential("200 millis"), times: 3 }),
    );
    const crypto = yield* Effect.service(Crypto.Crypto);
    const claim = Effect.fn("CoordinatorClient.claim")(function* () {
      const response = yield* ordinary.execute(HttpClientRequest.post("/v1/claim").pipe(
        HttpClientRequest.bodyJsonUnsafe({ workerId: options.workerId, claimId: yield* crypto.randomUUIDv4 } satisfies WorkerClaimRequest),
      ));
      return response.status === 204 ? undefined : yield* HttpClientResponse.schemaBodyJson(ClaimAssignment)(response);
    }, Effect.mapError((cause) => ExecutorError.from("claim job", String(cause), cause)));

    const heartbeat = Effect.fn("CoordinatorClient.heartbeat")(function* (job: JobRequest) {
      yield* ordinary.execute(HttpClientRequest.post(`/v1/jobs/${encodeURIComponent(job.jobId)}/heartbeat`).pipe(
        HttpClientRequest.bodyJsonUnsafe({ jobId: job.jobId, generation: job.generation, attempt: job.attempt, leaseToken: job.leaseToken }),
      ));
    }, Effect.mapError((cause) => ExecutorError.from("heartbeat", String(cause), cause)));
    const complete = Effect.fn("CoordinatorClient.complete")(function* (record: JobRecord) {
      yield* completion.execute(HttpClientRequest.post(`/v1/jobs/${encodeURIComponent(record.request.jobId)}/complete`).pipe(
        HttpClientRequest.bodyJsonUnsafe(completionFor(record)),
      ));
    }, Effect.mapError((cause) => ExecutorError.from("complete job", String(cause), cause)));
    return CoordinatorClient.of({ claim, heartbeat, complete });
  })).pipe(Layer.provide(Layer.merge(FetchHttpClient.layer, BunCrypto.layer)));

export { jobRequestOf };
