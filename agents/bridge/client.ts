import { Effect, Schedule, Schema, Stream } from "effect";
import { FetchHttpClient, HttpClient, HttpClientRequest } from "effect/unstable/http";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import { GitObjectId, JobSpec, PublicStoredJob } from "../src/contracts.ts";
import { BridgeError } from "./errors.ts";
import { AnomaliesResponse, JsonObject } from "./types.ts";

const failure = (operation: string, detail: string, retryable = false) =>
  new BridgeError({ operation, detail, retryable });
const decode = <S extends Schema.Top & { readonly DecodingServices: never }>(
  schema: S, value: unknown, operation: string,
) => Schema.decodeUnknownEffect(schema)(value).pipe(
  Effect.mapError(() => failure(operation, "response does not match the expected schema")),
);
const DiagnosisResponse = Schema.Struct({
  results: Schema.Record(Schema.String, JsonObject),
  errors: Schema.Record(Schema.String, Schema.String),
});
const DiagnosisIdentity = Schema.Struct({
  service: Schema.String,
  window: Schema.Struct({ from_ts: Schema.Int, to_ts: Schema.Int }),
});
const PullRequest = Schema.Struct({ state: Schema.Literals(["open", "closed"]), merged: Schema.Boolean });

export const makeBridgeClient = Effect.fn("makeBridgeClient")(function* (options: {
  sigiroUrl: string; sigiroAccessToken: string; coordinatorUrl: string; workerToken: string;
}) {
  // DI: capture existing transport/process capabilities without another service layer.
  const client = HttpClient.withScope(yield* HttpClient.HttpClient);
  const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
  const boundedText = <E>(stream: Stream.Stream<Uint8Array, E>, limit: number, operation: string) =>
    stream.pipe(
      Stream.runFoldEffect(() => ({ size: 0, chunks: [] as Uint8Array[] }), (state, chunk) => {
        if (state.size + chunk.byteLength > limit) {
          return Effect.fail(failure(operation, `response exceeds ${limit} bytes`));
        }
        state.size += chunk.byteLength;
        state.chunks.push(chunk);
        return Effect.succeed(state);
      }),
      Effect.map(({ size, chunks }) => {
        const bytes = new Uint8Array(size);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        return new TextDecoder().decode(bytes);
      }),
    );
  const parseJson = (text: string, operation: string, retryable = false) => Effect.try({
    try: () => JSON.parse(text) as unknown,
    catch: () => failure(operation, "response is not valid JSON", retryable),
  });
  const request = Effect.fn("BridgeClient.request")(function* (
    operation: string, request: HttpClientRequest.HttpClientRequest, limit: number, allowMissing = false,
  ) {
    const response = yield* client.execute(request).pipe(
      Effect.mapError(() => failure(operation, "HTTP transport failed", true)),
    );
    if (allowMissing && response.status === 404) return undefined;
    if (response.status < 200 || response.status >= 300) {
      return yield* failure(operation, `HTTP ${response.status}`, response.status === 429 || response.status >= 500);
    }
    const text = yield* boundedText(response.stream, limit, operation).pipe(
      Effect.mapError((error) => error instanceof BridgeError ? error : failure(operation, "response stream failed", true)),
    );
    // A successful create may have committed even when its acknowledgement is unreadable.
    return yield* parseJson(text, operation, operation === "submit job");
  },
  Effect.scoped,
  Effect.provideService(FetchHttpClient.RequestInit, { redirect: "error" }),
  Effect.timeout("30 seconds"),
  Effect.catchTag("TimeoutError", () => Effect.fail(failure("HTTP request", "timed out after 30 seconds", true))),
  Effect.retry({ times: 3, schedule: Schedule.exponential("200 millis"), while: (error) => error.retryable }));

  const sigiro = (path: string) => `${options.sigiroUrl.replace(/\/$/, "")}${path}`;
  const coordinator = (path: string) => `${options.coordinatorUrl.replace(/\/$/, "")}${path}`;
  const anomalies = Effect.fn("BridgeClient.anomalies")(function* (service: string) {
    const response = yield* request("read anomalies", HttpClientRequest.get(sigiro("/v1/anomalies"), {
      urlParams: { service },
    }).pipe(HttpClientRequest.bearerToken(options.sigiroAccessToken)), 1024 * 1024);
    const result = yield* decode(AnomaliesResponse, response, "read anomalies");
    if (result.truncated) return yield* failure("read anomalies", "anomaly response is truncated; narrow the configured service");
    if (result.anomalies.some((row) => row.service_name !== service)) {
      return yield* failure("read anomalies", "response includes an unexpected service");
    }
    return result.anomalies;
  });
  const diagnose = Effect.fn("BridgeClient.diagnose")(function* (service: string, fromTs: number, toTs: number) {
    const response = yield* request("diagnose", HttpClientRequest.post(sigiro("/v1/diagnose")).pipe(
      HttpClientRequest.bearerToken(options.sigiroAccessToken),
      HttpClientRequest.bodyJsonUnsafe({ services: [service], from_ts: fromTs, to_ts: toTs }),
    ), 2 * 1024 * 1024);
    const envelope = yield* decode(DiagnosisResponse, response, "diagnose");
    if (Object.hasOwn(envelope.errors, service)) return yield* failure("diagnose", "service diagnosis reported an error", true);
    const diagnosis = envelope.results[service];
    const identity = yield* decode(DiagnosisIdentity, diagnosis, "diagnose");
    if (identity.service !== service || identity.window.from_ts !== fromTs || identity.window.to_ts !== toTs) {
      return yield* failure("diagnose", "service or diagnosis window does not match the request");
    }
    return diagnosis!;
  });
  const github = Effect.fn("BridgeClient.github")(function* (endpoint: string) {
    const handle = yield* spawner.spawn(ChildProcess.make("gh", ["api", "--hostname", "github.com", "--method", "GET", endpoint], {
      stdin: "ignore", stdout: "pipe", stderr: "pipe", extendEnv: true, env: { GH_PROMPT_DISABLED: "1" },
    })).pipe(Effect.mapError(() => failure("GitHub", "could not start gh")));
    const [stdout, stderr, exitCode] = yield* Effect.all([
      boundedText(handle.stdout, 1024 * 1024, "GitHub"),
      boundedText(handle.stderr, 64 * 1024, "GitHub"),
      handle.exitCode,
    ], { concurrency: "unbounded" }).pipe(
      Effect.mapError((error) => error instanceof BridgeError ? error : failure("GitHub", "could not read gh result", true)),
    );
    if (exitCode !== ChildProcessSpawner.ExitCode(0)) {
      // Only expose the numeric HTTP status, never remote response text or credentials.
      const httpStatus = /\(HTTP (\d{3})\)/.exec(stderr)?.[1];
      const retryable = httpStatus === undefined
        ? exitCode !== ChildProcessSpawner.ExitCode(4)
        : httpStatus === "429" || Number(httpStatus) >= 500;
      return yield* failure("GitHub", `gh exited with status ${exitCode}${httpStatus ? ` (HTTP ${httpStatus})` : ""}`, retryable);
    }
    return yield* parseJson(stdout, "GitHub");
  }, Effect.scoped, Effect.timeout("30 seconds"),
  Effect.catchTag("TimeoutError", () => Effect.fail(failure("GitHub", "gh timed out after 30 seconds", true))));
  const repositoryPath = (repository: string) => repository.split("/").map(encodeURIComponent).join("/");
  const baseSha = Effect.fn("BridgeClient.baseSha")(function* (repository: string, baseRef: string) {
    const branch = baseRef.replace(/^refs\/heads\//, "");
    const response = yield* github(`repos/${repositoryPath(repository)}/git/ref/heads/${encodeURIComponent(branch)}`);
    return (yield* decode(Schema.Struct({ object: Schema.Struct({ sha: GitObjectId }) }), response, "resolve base SHA")).object.sha;
  });
  const submit = Effect.fn("BridgeClient.submit")(function* (job: JobSpec) {
    const response = yield* request("submit job", HttpClientRequest.post(coordinator("/v1/jobs")).pipe(
      HttpClientRequest.bearerToken(options.workerToken), HttpClientRequest.bodyJsonUnsafe(job),
    ), 1024 * 1024);
    const stored = yield* decode(PublicStoredJob, response, "submit job").pipe(
      Effect.mapError((error) => failure(error.operation, error.detail, true)),
    );
    const expected = yield* decode(JobSpec, job, "submit job");
    if (JSON.stringify(stored.spec) !== JSON.stringify(expected)) {
      return yield* failure("submit job", "coordinator returned different immutable job input");
    }
    return stored;
  });
  const status = Effect.fn("BridgeClient.status")(function* (jobId: string) {
    const response = yield* request("read job", HttpClientRequest.get(coordinator(`/v1/jobs/${encodeURIComponent(jobId)}`)).pipe(
      HttpClientRequest.bearerToken(options.workerToken),
    ), 1024 * 1024, true);
    if (response === undefined) return undefined;
    const job = yield* decode(PublicStoredJob, response, "read job");
    if (job.spec.jobId !== jobId) return yield* failure("read job", "coordinator returned a different job");
    return job;
  });
  const pullRequest = Effect.fn("BridgeClient.pullRequest")(function* (repository: string, number: number) {
    const response = yield* github(`repos/${repositoryPath(repository)}/pulls/${number}`);
    return yield* decode(PullRequest, response, "read pull request");
  });
  return { anomalies, diagnose, baseSha, submit, status, pullRequest };
});

export type BridgeClient = Effect.Success<ReturnType<typeof makeBridgeClient>>;
