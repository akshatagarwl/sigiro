import { Client, type MessageStreamEvent } from "eve/client";
import { Context, Effect, Layer, Predicate, Schema } from "effect";
import { JobStore } from "./services.ts";
import type { JobRecord, JobRequest } from "./types.ts";
import { executionId } from "./types.ts";

export interface EveResult {
  readonly summary: string;
  readonly sessionId: string;
  readonly branch?: string;
  readonly commitSha?: string;
  readonly changedPaths?: ReadonlyArray<string>;
  readonly pullRequestNumber?: number;
  readonly pullRequestUrl?: string;
}

/**
 * The publication receipt the GitHub `turn.completed` handler wrote, as the
 * result route serves it.
 *
 * Eve's `callAdapterEventHandler` swallows a throwing adapter event handler and
 * writes the event anyway, so an observed `turn.completed` proves only that the
 * turn ended. Only a `published` receipt proves the required tests ran and the
 * draft PR exists.
 */
export interface PublicationReceipt {
  readonly executionId: string;
  readonly status: "published" | "failed";
  readonly error?: string;
  readonly branch?: string;
  readonly commitSha?: string;
  readonly changedPaths?: ReadonlyArray<string>;
  readonly pullRequestNumber?: number;
  readonly pullRequestUrl?: string;
}

/**
 * Raised when the run exceeded its deadline and the turn was cancelled. It is
 * distinct from a plain failure so the worker never has to read an error
 * message to learn why a run ended.
 */
export class EveDeadlineExceeded extends Schema.TaggedError<EveDeadlineExceeded>()("EveDeadlineExceeded", {
  sessionId: Schema.String,
  turnId: Schema.optionalKey(Schema.String),
}) {
  override get message(): string {
    return `Eve deadline exceeded for session ${this.sessionId}`;
  }
}

export class EveFailure extends Schema.TaggedError<EveFailure>()("EveFailure", {
  detail: Schema.String,
  cause: Schema.optionalKey(Schema.Defect()),
}) {
  constructor(detail: string, cause?: unknown) {
    super(cause === undefined ? { detail } : { detail, cause });
  }

  override get message(): string {
    return this.detail;
  }
}

export type EveError = EveDeadlineExceeded | EveFailure;

export class EveExecution extends Context.Service<EveExecution, {
  readonly run: (request: JobRequest) => Effect.Effect<EveResult, EveError>;
  readonly cancel: (sessionId: string) => Effect.Effect<void, EveError>;
}>()("sigiro/agents/EveExecution") {}

/** Options accepted by a transport stream read. */
export interface EveStreamOptions {
  readonly startIndex: number;
  readonly follow: boolean;
  readonly signal: AbortSignal;
}

/**
 * The slice of one attached Eve session this executor actually uses.
 *
 * `eve/client`'s `Client` builds its own requests with the global fetch and
 * exposes no injection point, so tests cannot drive the real class without a
 * live server. Naming the four operations the executor needs keeps the
 * production path on the real client while letting tests supply a fake.
 */
export interface EveSession {
  readonly sessionId: string;
  /** Cursor of the next unread event, advanced by the transport as it yields. */
  readonly streamIndex: () => number;
  readonly stream: (options: EveStreamOptions) => AsyncIterable<MessageStreamEvent>;
  readonly cancel: (turnId: string) => Promise<void>;
  readonly snapshot: () => Promise<{ readonly events: ReadonlyArray<MessageStreamEvent>; readonly streamIndex: number }>;
}

export interface EveTransport {
  /** Starts a new run and returns its session id. */
  readonly dispatch: (request: JobRequest) => Promise<string>;
  /** Binds to an existing session id without performing I/O. */
  readonly attach: (sessionId: string, streamIndex: number) => EveSession;
  /** Reads the publication receipt, or `undefined` while none has been written. */
  readonly fetchResult: (executionId: string) => Promise<PublicationReceipt | undefined>;
}

const turnOf = (event: MessageStreamEvent): string | undefined => {
  const data = (event as { data?: { turnId?: unknown } }).data;
  return typeof data?.turnId === "string" ? data.turnId : undefined;
};

const messageOf = (event: MessageStreamEvent): string | undefined => {
  const data = (event as { data?: { message?: unknown } }).data;
  return typeof data?.message === "string" ? data.message : undefined;
};

const failureEvents = new Set(["session.failed", "turn.failed", "step.failed"]);

/** How long a cancellation may take to be confirmed before the run gives up. */
const confirmMs = 5_000;

/**
 * The receipt is written by the `turn.completed` handler, which Eve awaits
 * before writing the event, but the write and the worker's read are separate
 * processes on separate clocks. These bound the wait for it to appear.
 */
const receiptPollIntervalMs = 2_000;
const receiptPollTimeoutMs = 30_000;
const receiptPollAttempts = Math.ceil(receiptPollTimeoutMs / receiptPollIntervalMs);

export interface EveExecutionOptions {
  readonly url: string;
  readonly workerToken: string;
  readonly workerId: string;
}

const optionalString = (value: unknown): string | undefined => typeof value === "string" ? value : undefined;

/**
 * Validates a receipt off the wire. An unrecognized shape is not a receipt, and
 * treating it as "still pending" keeps a malformed response from deciding an
 * execution's fate.
 */
export const parseReceipt = (value: unknown): PublicationReceipt | undefined => {
  if (typeof value !== "object" || value === null) return undefined;
  const record = value as Record<string, unknown>;
  if (record.status !== "published" && record.status !== "failed") return undefined;
  const executionId = optionalString(record.executionId);
  if (executionId === undefined) return undefined;
  const changedPaths = Array.isArray(record.changedPaths) && record.changedPaths.every((entry) => typeof entry === "string")
    ? record.changedPaths as ReadonlyArray<string>
    : undefined;
  const pullRequestNumber = typeof record.pullRequestNumber === "number" && Number.isInteger(record.pullRequestNumber)
    ? record.pullRequestNumber
    : undefined;
  const error = optionalString(record.error);
  const branch = optionalString(record.branch);
  const commitSha = optionalString(record.commitSha);
  const pullRequestUrl = optionalString(record.pullRequestUrl);
  return {
    executionId,
    status: record.status,
    ...(error === undefined ? {} : { error }),
    ...(branch === undefined ? {} : { branch }),
    ...(commitSha === undefined ? {} : { commitSha }),
    ...(changedPaths === undefined ? {} : { changedPaths }),
    ...(pullRequestNumber === undefined ? {} : { pullRequestNumber }),
    ...(pullRequestUrl === undefined ? {} : { pullRequestUrl }),
  };
};

/** The default transport, backed by the real `eve/client` `Client`. */
export const clientTransport = (options: EveExecutionOptions): EveTransport => {
  const client = new Client({ host: options.url, auth: { bearer: options.workerToken }, redirect: "error" });
  return {
    dispatch: async (request) => {
      const response = await client.fetch("/internal/celld/job", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          executionId: executionId(request), workerId: options.workerId, repository: request.repository,
          baseRef: request.baseRef, baseSha: request.baseSha, prompt: request.prompt, tests: request.tests,
          allowedPaths: request.allowedPaths, github: request.github,
        }),
      });
      if (!response.ok) throw new Error(`Eve dispatch failed: HTTP ${response.status}`);
      const body = await response.json() as { sessionId?: unknown };
      if (typeof body.sessionId !== "string" || body.sessionId.length === 0) throw new Error("Eve dispatch returned no session");
      return body.sessionId;
    },
    fetchResult: async (id) => {
      const response = await client.fetch("/internal/celld/job/result", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ executionId: id }),
      });
      // 404 is the route's "no receipt yet" answer, not an error.
      if (response.status === 404) return undefined;
      if (!response.ok) throw new Error(`Eve result read failed: HTTP ${response.status}`);
      return parseReceipt(await response.json());
    },
    attach: (sessionId, streamIndex) => {
      const session = client.sessions.attach(sessionId, { streamIndex });
      return {
        sessionId,
        streamIndex: () => session.state.streamIndex,
        stream: (streamOptions) => session.stream(streamOptions),
        cancel: async (turnId) => { await session.cancel({ turnId }); },
        snapshot: async () => {
          const snapshot = await session.snapshot();
          return { events: snapshot.events, streamIndex: snapshot.session.streamIndex };
        },
      };
    },
  };
};

/** Result of consuming the stream up to a boundary the executor recognizes. */
type StreamOutcome =
  | { readonly kind: "succeeded"; readonly summary: string }
  | { readonly kind: "failed"; readonly detail: string };

const asFailure = (cause: unknown): EveFailure =>
  cause instanceof EveFailure ? cause : new EveFailure(cause instanceof Error ? cause.message : String(cause), cause);

export const makeEveExecutionWith = (transport: EveTransport): Layer.Layer<EveExecution, never, JobStore> =>
  Layer.effect(EveExecution, Effect.gen(function*() {
    const jobs = yield* Effect.service(JobStore);

    const dispatch = (request: JobRequest) =>
      Effect.tryPromise({ try: () => transport.dispatch(request), catch: asFailure });

    const sessionFor = Effect.fn("EveExecution.sessionFor")(function*(record: JobRecord) {
      if (record.eveSessionId !== undefined) {
        return transport.attach(record.eveSessionId, record.eveStreamIndex ?? 0);
      }
      const sessionId = yield* dispatch(record.request);
      // This is necessarily a dispatch-response-to-local-write crash window. Stable publicationId
      // makes GitHub effects idempotent; a duplicate model turn can still spend subscription quota.
      const current = yield* jobs.get(record.executionId).pipe(Effect.mapError(asFailure));
      if (Predicate.isUndefined(current)) {
        return yield* new EveFailure("job disappeared before Eve session persistence");
      }
      yield* jobs.put({ ...current, eveSessionId: sessionId, eveStreamIndex: 0, updatedAt: new Date().toISOString() })
        .pipe(Effect.mapError(asFailure));
      return transport.attach(sessionId, 0);
    });

    /**
     * Consumes events until a boundary, then reports what it saw. Cursor
     * progress is handed back rather than written here: persistence belongs in
     * Effect land, and a fresh runtime per event is what F14 was about.
     */
    const consume = async (
      session: EveSession,
      signal: AbortSignal,
      onEvent: (event: MessageStreamEvent, cursor: number) => void,
    ): Promise<StreamOutcome> => {
      let summary = "";
      let completed = false;
      for await (const event of session.stream({ startIndex: session.streamIndex(), follow: true, signal })) {
        onEvent(event, session.streamIndex());
        if (event.type === "message.completed") summary = messageOf(event) ?? summary;
        if (event.type === "turn.completed") completed = true;
        if (event.type === "turn.cancelled") completed = false;
        if (failureEvents.has(event.type)) return { kind: "failed", detail: `Eve ${event.type}` };
        if (event.type === "session.waiting") {
          return completed ? { kind: "succeeded", summary } : { kind: "failed", detail: "Eve turn did not complete" };
        }
        if (event.type === "session.completed") {
          return completed ? { kind: "succeeded", summary } : { kind: "failed", detail: "Eve session completed without turn" };
        }
      }
      return { kind: "failed", detail: "Eve stream ended before a terminal boundary" };
    };

    /**
     * Waits for `turn.cancelled` on `turnId` followed by `session.waiting`.
     * The caller's abort controller stops the underlying read, so the timer
     * winning never leaves a consumer attached to the stream.
     */
    const confirmCancelled = async (session: EveSession, turnId: string, signal: AbortSignal): Promise<boolean> => {
      let cancelled = false;
      for await (const event of session.stream({ startIndex: session.streamIndex(), follow: true, signal })) {
        if (event.type === "turn.cancelled" && turnOf(event) === turnId) cancelled = true;
        if (cancelled && event.type === "session.waiting") return true;
      }
      return false;
    };

    /**
     * Runs `work` against its own abort controller and aborts it as soon as the
     * bound elapses, so the timer never races an orphaned reader.
     */
    const bounded = Effect.fn("EveExecution.bounded")(
      function*<A>(work: (signal: AbortSignal) => Promise<A>, onTimeout: A) {
        const controller = new AbortController();
        return yield* Effect.tryPromise({ try: () => work(controller.signal), catch: asFailure }).pipe(
          Effect.timeoutOrElse({
            duration: `${confirmMs} millis`,
            orElse: () => Effect.sync(() => { controller.abort(); }).pipe(Effect.as(onTimeout)),
          }),
          Effect.ensuring(Effect.sync(() => { controller.abort(); })),
        );
      },
    );

    const cancelTurn = Effect.fn("EveExecution.cancelTurn")(function*(session: EveSession, turnId: string) {
      yield* Effect.tryPromise({ try: () => session.cancel(turnId), catch: asFailure });
      return yield* bounded((signal) => confirmCancelled(session, turnId, signal), false);
    });

    /**
     * Polls the result route until a receipt exists, bounded. The first read
     * happens immediately, because the handler is awaited before Eve writes
     * `turn.completed` and the receipt is usually already there.
     */
    const awaitReceipt = Effect.fn("EveExecution.awaitReceipt")(
      function*(id: string): Effect.fn.Return<PublicationReceipt, EveError> {
        for (let attempt = 0; attempt < receiptPollAttempts; attempt += 1) {
          if (attempt > 0) yield* Effect.sleep(`${receiptPollIntervalMs} millis`);
          const receipt = yield* Effect.tryPromise({ try: () => transport.fetchResult(id), catch: asFailure });
          if (!Predicate.isUndefined(receipt)) return receipt;
        }
        return yield* new EveFailure(
          `Eve turn completed but no publication receipt appeared for ${id} within ${receiptPollTimeoutMs} ms; publication status is unknown because Eve swallows a throwing turn.completed handler`,
        );
      },
    );

    const run = Effect.fn("EveExecution.run")(function*(request: JobRequest) {
      const id = executionId(request);
      const record = yield* jobs.get(id).pipe(Effect.mapError(asFailure));
      if (Predicate.isUndefined(record)) return yield* new EveFailure("job disappeared");
      const session = yield* sessionFor(record);

      // Observed as the stream advances, then read by the deadline path.
      let turnId: string | undefined;
      let cursor = record.eveStreamIndex ?? 0;
      let persisted = cursor;

      const saveCursor = Effect.suspend(() =>
        cursor <= persisted ? Effect.void : jobs.advanceCursor(id, cursor).pipe(
          Effect.tap(Effect.sync(() => { persisted = cursor; })),
          Effect.mapError(asFailure),
        )
      );

      const controller = new AbortController();
      // Aborting the stream makes it settle promptly, and a plain race would
      // then hand back that abort-induced "stream ended" instead of the
      // deadline outcome that caused it. The latch parks the loser forever so
      // the deadline branch is the only one that can complete the race.
      let expired = false;
      const streamed = Effect.tryPromise({
        try: () => consume(session, controller.signal, (event, next) => {
          turnId = turnOf(event) ?? turnId;
          cursor = Math.max(cursor, next);
        }),
        catch: asFailure,
      }).pipe(
        // The cursor is durable whether the read ended at a boundary or threw.
        Effect.ensuring(saveCursor.pipe(Effect.ignore)),
        Effect.onInterrupt(() => Effect.sync(() => { controller.abort(); })),
        Effect.flatMap((outcome): Effect.Effect<StreamOutcome, EveError> =>
          expired ? Effect.never : Effect.succeed(outcome)
        ),
        Effect.catchIf(() => expired, () => Effect.never),
      );

      // The deadline is a race, not a message match: a transient network error
      // from the stream is a plain failure and must not trigger cancellation.
      const deadline: Effect.Effect<StreamOutcome, EveError> = Effect.sleep(
        `${Math.max(1, request.deadlineSeconds)} seconds`,
      ).pipe(
        Effect.andThen(Effect.sync(() => { expired = true; controller.abort(); })),
        Effect.andThen(Effect.suspend((): Effect.Effect<StreamOutcome, EveError> => {
          const active = turnId;
          if (active === undefined) {
            return Effect.fail(new EveFailure("Eve deadline reached without active turn"));
          }
          return cancelTurn(session, active).pipe(
            Effect.flatMap((confirmed): Effect.Effect<StreamOutcome, EveError> =>
              Effect.fail(confirmed
                ? new EveDeadlineExceeded({ sessionId: session.sessionId, turnId: active })
                : new EveFailure("Eve cancellation was not confirmed"))
            ),
          );
        })),
      );

      const outcome: StreamOutcome = yield* Effect.raceFirst(streamed, deadline);
      if (outcome.kind === "failed") return yield* new EveFailure(outcome.detail);

      // `turn.completed` proves only that the turn ended: Eve swallows a
      // throwing adapter event handler and stamps the event regardless. The
      // receipt is the only evidence that the handler ran the required tests and
      // published, so the run is not successful until one says "published".
      const receipt = yield* awaitReceipt(id);
      if (receipt.status === "failed") {
        return yield* new EveFailure(`Eve publication failed: ${receipt.error ?? "no reason recorded"}`);
      }
      return {
        summary: outcome.summary,
        sessionId: session.sessionId,
        ...(receipt.branch === undefined ? {} : { branch: receipt.branch }),
        ...(receipt.commitSha === undefined ? {} : { commitSha: receipt.commitSha }),
        ...(receipt.changedPaths === undefined ? {} : { changedPaths: receipt.changedPaths }),
        ...(receipt.pullRequestNumber === undefined ? {} : { pullRequestNumber: receipt.pullRequestNumber }),
        ...(receipt.pullRequestUrl === undefined ? {} : { pullRequestUrl: receipt.pullRequestUrl }),
      } satisfies EveResult;
    });

    const cancel = Effect.fn("EveExecution.cancel")(function*(sessionId: string) {
      const session = transport.attach(sessionId, 0);
      const snapshot = yield* Effect.tryPromise({ try: () => session.snapshot(), catch: asFailure });
      let turnId: string | undefined;
      for (const event of snapshot.events) turnId = turnOf(event) ?? turnId;
      if (turnId === undefined) return yield* new EveFailure("Eve active turn not found");
      const confirmed = yield* cancelTurn(session, turnId);
      if (!confirmed) return yield* new EveFailure("Eve cancellation was not confirmed");
    });

    return EveExecution.of({ run, cancel });
  }));

export const makeEveExecution = (options: EveExecutionOptions): Layer.Layer<EveExecution, never, JobStore> =>
  makeEveExecutionWith(clientTransport(options));
