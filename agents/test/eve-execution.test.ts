import { expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { MessageStreamEvent } from "eve/client";
import { Effect, Layer } from "effect";
import {
  EveExecution,
  type EveSession,
  type EveStreamOptions,
  type EveTransport,
  makeEveExecutionWith,
  parseReceipt,
  type PublicationReceipt,
} from "../executor/eve.ts";
import { JobStore, makeFileJobStore } from "../executor/services.ts";
import type { JobRequest } from "../executor/types.ts";

const run = <A, E>(effect: Effect.Effect<A, E, never>) => Effect.runPromise(effect);

const request: JobRequest = {
  jobId: "job-1",
  generation: 1,
  attempt: 1,
  repository: "/repo",
  github: { installationId: 1, issueNumber: 1 },
  baseRef: "main",
  baseSha: "abc",
  prompt: "fix",
  tests: [],
  allowedPaths: ["**"],
  deadlineSeconds: 30,
  leaseToken: "lease-1",
};

/** Builds an event of the shape the executor reads: `type` plus a `data` bag. */
const event = (type: string, data: Record<string, unknown> = {}): MessageStreamEvent =>
  ({ type, data }) as unknown as MessageStreamEvent;

interface FakeSessionScript {
  /** Events yielded by the first `stream` call. */
  readonly events: ReadonlyArray<MessageStreamEvent>;
  /** Events yielded by every `stream` call opened after a cancel request. */
  readonly afterCancel?: ReadonlyArray<MessageStreamEvent>;
  /** Held open without yielding, to model a turn that never reaches a boundary. */
  readonly hang?: boolean;
  /** Never yields the confirmation, to model a cancel that is not acknowledged. */
  readonly hangAfterCancel?: boolean;
  readonly snapshot?: ReadonlyArray<MessageStreamEvent>;
  /**
   * Receipt served by the result route. `undefined` models a handler that never
   * wrote one, which the run must refuse rather than report as success.
   */
  readonly receipt?: PublicationReceipt;
  /** Reads that answer "pending" before `receipt` starts being served. */
  readonly pendingReads?: number;
}

const publishedReceipt: PublicationReceipt = {
  executionId: "job-1:1:1",
  status: "published",
  branch: "sigiro/eve-abc",
  commitSha: "c".repeat(40),
  changedPaths: ["src/a.ts"],
  pullRequestNumber: 3,
  pullRequestUrl: "https://example.test/pr/3",
};

interface FakeTransport extends EveTransport {
  readonly dispatches: () => number;
  readonly cancels: () => ReadonlyArray<string>;
  readonly attaches: () => ReadonlyArray<{ sessionId: string; streamIndex: number }>;
  /** Number of stream readers currently inside the loop. */
  readonly peakConcurrentStreams: () => number;
  readonly aborted: () => number;
  readonly resultReads: () => number;
}

/**
 * Waits for an abort, so a "hanging" stream still terminates when the run
 * aborts it. A stream that ignored the signal would hang the test instead of
 * proving the abort reached it.
 */
const untilAborted = (signal: AbortSignal): Promise<void> =>
  signal.aborted ? Promise.resolve() : new Promise((resolve) => {
    signal.addEventListener("abort", () => resolve(), { once: true });
  });

const fakeTransport = (script: FakeSessionScript, sessionId = "session-1"): FakeTransport => {
  let dispatches = 0;
  let cursor = 0;
  let cancelled = false;
  let live = 0;
  let peak = 0;
  let aborted = 0;
  let resultReads = 0;
  const cancels: Array<string> = [];
  const attaches: Array<{ sessionId: string; streamIndex: number }> = [];

  const session = (start: number): EveSession => {
    cursor = start;
    return {
      sessionId,
      streamIndex: () => cursor,
      // Each read yields whatever the script has left for the current phase, so
      // the absolute `startIndex` a resumed run passes in does not have to line
      // up with the fixture's array length.
      stream: (options: EveStreamOptions) => ({
        async *[Symbol.asyncIterator]() {
          live += 1;
          peak = Math.max(peak, live);
          try {
            const source = cancelled ? (script.afterCancel ?? []) : script.events;
            const hangs = cancelled ? script.hangAfterCancel === true : script.hang === true;
            for (const next of source) {
              if (options.signal.aborted) { aborted += 1; return; }
              cursor += 1;
              yield next;
            }
            if (hangs) {
              await untilAborted(options.signal);
              aborted += 1;
              return;
            }
          } finally {
            live -= 1;
          }
        },
      }),
      cancel: async (turnId: string) => {
        cancels.push(turnId);
        cancelled = true;
      },
      snapshot: async () => ({ events: script.snapshot ?? script.events, streamIndex: cursor }),
    };
  };

  return {
    dispatch: async () => {
      dispatches += 1;
      return sessionId;
    },
    attach: (id, streamIndex) => {
      attaches.push({ sessionId: id, streamIndex });
      return session(streamIndex);
    },
    fetchResult: async () => {
      resultReads += 1;
      if (resultReads <= (script.pendingReads ?? 0)) return undefined;
      return script.receipt ?? publishedReceipt;
    },
    dispatches: () => dispatches,
    cancels: () => cancels,
    attaches: () => attaches,
    peakConcurrentStreams: () => peak,
    aborted: () => aborted,
    resultReads: () => resultReads,
  };
};

/** Runs one `EveExecution.run` against a real on-disk job store. */
const withExecution = async <A>(
  transport: EveTransport,
  body: (eve: EveExecution["Service"], jobs: JobStore["Service"]) => Promise<A>,
  seed: (jobs: JobStore["Service"]) => Promise<void> = async () => {},
): Promise<A> => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-exec-"));
  try {
    const store = makeFileJobStore(root);
    const jobs = await Effect.runPromise(Effect.provide(Effect.service(JobStore), store));
    await run(jobs.create(request));
    await seed(jobs);
    const eve = await Effect.runPromise(
      Effect.provide(Effect.service(EveExecution), makeEveExecutionWith(transport).pipe(Layer.provide(store))),
    );
    return await body(eve, jobs);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
};

const completedRun: ReadonlyArray<MessageStreamEvent> = [
  event("turn.started", { turnId: "turn-1" }),
  event("message.completed", { turnId: "turn-1", message: "patched the thing" }),
  event("turn.completed", { turnId: "turn-1" }),
  event("session.waiting", { turnId: "turn-1" }),
];

test("a first run dispatches and persists the session id before streaming", async () => {
  const transport = fakeTransport({ events: completedRun });
  await withExecution(transport, async (eve, jobs) => {
    const result = await run(eve.run(request));
    expect(result.summary).toBe("patched the thing");
    expect(result.sessionId).toBe("session-1");
    // The summary stays the model's message; the receipt supplies the artifact.
    expect(result.pullRequestUrl).toBe("https://example.test/pr/3");
    expect(result.pullRequestNumber).toBe(3);
    expect(result.branch).toBe("sigiro/eve-abc");
    expect(result.commitSha).toBe("c".repeat(40));
    expect(result.changedPaths).toEqual(["src/a.ts"]);
    expect(transport.dispatches()).toBe(1);

    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.eveSessionId).toBe("session-1");
  });
});

/**
 * The session id is written before the stream is read, so a crash between
 * dispatch and the first event still leaves a resumable record rather than
 * orphaning a running turn.
 */
test("the session id is persisted before the first event is read", async () => {
  const base = fakeTransport({ events: completedRun });
  const order: Array<string> = [];
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-order-"));
  try {
    const store = makeFileJobStore(root);
    const jobs = await Effect.runPromise(Effect.provide(Effect.service(JobStore), store));
    await run(jobs.create(request));

    // The store write is the only thing that can happen between these two
    // marks, so a `persist` entry before `stream` is the ordering under test.
    const observing: EveTransport = {
      dispatch: async (input) => {
        order.push("dispatch");
        return base.dispatch(input);
      },
      fetchResult: base.fetchResult,
      attach: (id, index) => {
        const session = base.attach(id, index);
        return {
          ...session,
          stream: (options) => {
            order.push("stream");
            return session.stream(options);
          },
        };
      },
    };

    const put = jobs.put;
    const recording: JobStore["Service"] = {
      ...jobs,
      put: (record) => {
        if (record.eveSessionId !== undefined) order.push("persist");
        return put(record);
      },
    };
    const recordingStore = Layer.succeed(JobStore, JobStore.of(recording));

    const eve = await Effect.runPromise(
      Effect.provide(
        Effect.service(EveExecution),
        makeEveExecutionWith(observing).pipe(Layer.provide(recordingStore)),
      ),
    );
    await run(eve.run(request));
    expect(order).toEqual(["dispatch", "persist", "stream"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a restart with a persisted session attaches without dispatching", async () => {
  const transport = fakeTransport({ events: completedRun });
  await withExecution(
    transport,
    async (eve) => {
      await run(eve.run(request));
      expect(transport.dispatches()).toBe(0);
      expect(transport.attaches()[0]).toEqual({ sessionId: "session-1", streamIndex: 6 });
    },
    async (jobs) => {
      const current = await run(jobs.get("job-1:1:1"));
      if (current === undefined) throw new Error("fixture missing");
      await run(jobs.put({ ...current, eveSessionId: "session-1", eveStreamIndex: 6, status: "running" }));
    },
  );
});

test("session.waiting without turn.completed fails", async () => {
  const transport = fakeTransport({
    events: [event("turn.started", { turnId: "turn-1" }), event("session.waiting", { turnId: "turn-1" })],
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve turn did not complete");
  });
});

test("a cancelled turn followed by session.waiting fails", async () => {
  const transport = fakeTransport({
    events: [
      event("turn.started", { turnId: "turn-1" }),
      event("turn.completed", { turnId: "turn-1" }),
      event("turn.cancelled", { turnId: "turn-1" }),
      event("session.waiting", { turnId: "turn-1" }),
    ],
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve turn did not complete");
  });
});

test("session.completed without a completed turn fails", async () => {
  const transport = fakeTransport({
    events: [event("turn.started", { turnId: "turn-1" }), event("session.completed", { turnId: "turn-1" })],
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve session completed without turn");
  });
});

for (const failure of ["session.failed", "turn.failed", "step.failed"]) {
  test(`${failure} fails the run`, async () => {
    const transport = fakeTransport({
      events: [event("turn.started", { turnId: "turn-1" }), event(failure, { turnId: "turn-1" })],
    });
    await withExecution(transport, async (eve) => {
      const exit = await Effect.runPromiseExit(eve.run(request));
      expect(exit._tag).toBe("Failure");
      expect(String(exit)).toContain(`Eve ${failure}`);
    });
  });
}

test("a stream that ends before a boundary fails", async () => {
  const transport = fakeTransport({ events: [event("turn.started", { turnId: "turn-1" })] });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve stream ended before a terminal boundary");
  });
});

/**
 * F8. The deadline is a race against a timer, so reaching it cancels the
 * observed turn. F7/F18: the stream is aborted before the confirmation read
 * opens, so only one consumer is ever attached to the stream.
 */
test("the deadline cancels the observed turn and confirms it", async () => {
  const transport = fakeTransport({
    events: [event("turn.started", { turnId: "turn-7" })],
    hang: true,
    afterCancel: [event("turn.cancelled", { turnId: "turn-7" }), event("session.waiting", { turnId: "turn-7" })],
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run({ ...request, deadlineSeconds: 1 }));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("EveDeadlineExceeded");
    expect(transport.cancels()).toEqual(["turn-7"]);
    // The hung reader was aborted rather than left running beside the confirmation read.
    expect(transport.peakConcurrentStreams()).toBe(1);
    expect(transport.aborted()).toBeGreaterThan(0);
  });
});

test("a deadline with no observed turn fails without cancelling", async () => {
  const transport = fakeTransport({ events: [], hang: true });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run({ ...request, deadlineSeconds: 1 }));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve deadline reached without active turn");
    expect(transport.cancels()).toEqual([]);
  });
});

/**
 * The confirmation wait is bounded, and the bound aborts the underlying read
 * so the timer never wins beside a live consumer.
 */
test("an unconfirmed cancellation fails within the bounded window", async () => {
  const transport = fakeTransport({
    events: [event("turn.started", { turnId: "turn-9" })],
    hang: true,
    afterCancel: [],
    hangAfterCancel: true,
  });
  await withExecution(transport, async (eve) => {
    const started = Date.now();
    const exit = await Effect.runPromiseExit(eve.run({ ...request, deadlineSeconds: 1 }));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve cancellation was not confirmed");
    expect(transport.cancels()).toEqual(["turn-9"]);
    // 1s deadline plus the 5s confirmation bound, with headroom for scheduling.
    expect(Date.now() - started).toBeLessThan(12_000);
    expect(transport.peakConcurrentStreams()).toBe(1);
  });
}, 20_000);

/**
 * F14. The cursor is persisted from Effect land, so it advances monotonically
 * and survives the run without building a runtime per event.
 */
test("the cursor advances monotonically and is persisted", async () => {
  const transport = fakeTransport({ events: completedRun });
  await withExecution(transport, async (eve, jobs) => {
    await run(eve.run(request));
    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.eveStreamIndex).toBe(completedRun.length);
  });
});

test("the cursor is persisted even when the run fails", async () => {
  const transport = fakeTransport({
    events: [event("turn.started", { turnId: "turn-1" }), event("turn.failed", { turnId: "turn-1" })],
  });
  await withExecution(transport, async (eve, jobs) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.eveStreamIndex).toBe(2);
  });
});

test("cancel resolves the active turn from a snapshot and confirms it", async () => {
  const transport = fakeTransport({
    events: [],
    snapshot: [event("turn.started", { turnId: "turn-3" })],
    afterCancel: [event("turn.cancelled", { turnId: "turn-3" }), event("session.waiting", { turnId: "turn-3" })],
  });
  await withExecution(transport, async (eve) => {
    await run(eve.cancel("session-1"));
    expect(transport.cancels()).toEqual(["turn-3"]);
  });
});

test("cancel fails when no turn is present in the snapshot", async () => {
  const transport = fakeTransport({ events: [], snapshot: [] });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.cancel("session-1"));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve active turn not found");
  });
});

test("a transport dispatch failure surfaces as a plain failure", async () => {
  const transport: EveTransport = {
    dispatch: async () => { throw new Error("connection reset"); },
    attach: () => { throw new Error("must not attach"); },
    fetchResult: async () => { throw new Error("must not read a result"); },
  };
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("connection reset");
  });
});

/**
 * F8. A transient network error is a plain failure, not a deadline: it must not
 * trigger a cancel. The old code matched "timed out" in the message and
 * cancelled healthy turns whose transport happened to word an error that way.
 */
test("a network error mentioning a timeout does not trigger cancellation", async () => {
  const transport = fakeTransport({ events: [] });
  const failing: EveTransport = {
    dispatch: transport.dispatch,
    fetchResult: transport.fetchResult,
    attach: (id, index) => {
      const session = transport.attach(id, index);
      return {
        ...session,
        stream: () => ({
          async *[Symbol.asyncIterator]() {
            yield event("turn.started", { turnId: "turn-1" });
            throw new Error("socket connection timed out");
          },
        }),
      };
    },
  };
  await withExecution(failing, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("socket connection timed out");
    expect(transport.cancels()).toEqual([]);
  });
});

/**
 * Eve's `callAdapterEventHandler` wraps an authored handler in `try`/`catch`,
 * logs a throw, and writes the event anyway. So `turn.completed` followed by
 * `session.waiting` proves only that the turn ended. Success now requires the
 * durable publication receipt the handler wrote.
 */
test("a completed turn with no receipt inside the bound fails, naming the gap", async () => {
  const transport = fakeTransport({ events: completedRun, pendingReads: 1_000 });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("no publication receipt appeared for job-1:1:1");
    expect(String(exit)).toContain("swallows a throwing turn.completed handler");
    // Bounded: an immediate first read plus one per interval across 30s.
    expect(transport.resultReads()).toBe(15);
  });
}, 60_000);

test("a failed receipt fails the run with the recorded reason", async () => {
  const transport = fakeTransport({
    events: completedRun,
    receipt: { executionId: "job-1:1:1", status: "failed", error: "Required test failed: unit (exit 1)" },
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve publication failed: Required test failed: unit (exit 1)");
  });
});

test("a failed receipt with no recorded reason still fails", async () => {
  const transport = fakeTransport({
    events: completedRun,
    receipt: { executionId: "job-1:1:1", status: "failed" },
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("no reason recorded");
  });
});

/** The first read is immediate, so the common case costs no polling delay. */
test("a receipt already present is read once, without waiting", async () => {
  const transport = fakeTransport({ events: completedRun });
  await withExecution(transport, async (eve) => {
    const started = Date.now();
    await run(eve.run(request));
    expect(transport.resultReads()).toBe(1);
    expect(Date.now() - started).toBeLessThan(1_500);
  });
});

test("a receipt that arrives late is still accepted inside the bound", async () => {
  const transport = fakeTransport({ events: completedRun, pendingReads: 2 });
  await withExecution(transport, async (eve) => {
    const result = await run(eve.run(request));
    expect(result.pullRequestNumber).toBe(3);
    expect(transport.resultReads()).toBe(3);
  });
}, 30_000);

test("a run that never completes its turn never reads a receipt", async () => {
  const transport = fakeTransport({
    events: [event("turn.started", { turnId: "turn-1" }), event("session.waiting", { turnId: "turn-1" })],
  });
  await withExecution(transport, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(transport.resultReads()).toBe(0);
  });
});

test("a result route error surfaces as a plain failure", async () => {
  const base = fakeTransport({ events: completedRun });
  const failing: EveTransport = {
    dispatch: base.dispatch,
    attach: base.attach,
    fetchResult: async () => { throw new Error("Eve result read failed: HTTP 500"); },
  };
  await withExecution(failing, async (eve) => {
    const exit = await Effect.runPromiseExit(eve.run(request));
    expect(exit._tag).toBe("Failure");
    expect(String(exit)).toContain("Eve result read failed: HTTP 500");
  });
});

/** A malformed body is not a receipt, so it must not decide the execution. */
test("parseReceipt refuses anything that is not a receipt", () => {
  expect(parseReceipt(undefined)).toBeUndefined();
  expect(parseReceipt(null)).toBeUndefined();
  expect(parseReceipt("published")).toBeUndefined();
  expect(parseReceipt({ status: "pending" })).toBeUndefined();
  expect(parseReceipt({ status: "published" })).toBeUndefined();
  expect(parseReceipt({ executionId: "e", status: "elsewhere" })).toBeUndefined();
  expect(parseReceipt({ executionId: "e", status: "published" })).toEqual({ executionId: "e", status: "published" });
  expect(parseReceipt({
    executionId: "e", status: "published", branch: "b", commitSha: "c",
    changedPaths: ["a"], pullRequestNumber: 4, pullRequestUrl: "u", headSha: "h", sessionId: "s", updatedAt: "t",
  })).toEqual({
    executionId: "e", status: "published", branch: "b", commitSha: "c",
    changedPaths: ["a"], pullRequestNumber: 4, pullRequestUrl: "u",
  });
  // Wrong-typed optionals are dropped rather than carried through untyped.
  expect(parseReceipt({ executionId: "e", status: "failed", error: 7, changedPaths: [1], pullRequestNumber: 1.5 }))
    .toEqual({ executionId: "e", status: "failed" });
});
