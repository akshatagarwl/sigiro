import { expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Layer } from "effect";
import { CoordinatorClient, makeCoordinatorClient } from "../executor/coordinator.ts";
import { EveExecution, EveFailure } from "../executor/eve.ts";
import { drainUnreported, runOnce } from "../executor/worker.ts";
import { JobStore, makeFileJobStore } from "../executor/services.ts";
import type { ClaimAssignment } from "../src/contracts.ts";
import type { JobRecord, JobRequest } from "../executor/types.ts";

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

const assignment: ClaimAssignment = {
  job: {
    ...request,
    incidentId: "incident-1",
    evidenceUri: "evidence://1",
    evidenceSha256: "0".repeat(64),
    policy: {
      maxAttempts: 2,
      deadlineSeconds: 30,
      allowedPaths: ["**"],
      tests: [],
    },
  },
  attempt: 1,
  executionId: "job-1:1:1",
  leaseToken: "lease-1",
  leaseExpiresAt: new Date().toISOString(),
};

const jobRecordFixture = (overrides: Partial<JobRecord> = {}): JobRecord => ({
  executionId: "job-1:1:1",
  requestHash: "hash",
  request,
  status: "succeeded",
  acceptedAt: "now",
  updatedAt: "now",
  result: {
    patchSha256: "0".repeat(64),
    providerOutput: "done",
    tests: [],
  },
  ...overrides,
});

const clientFor = (
  responses: Array<Response | Error>,
  seen: Request[] = [],
  sleep = async (_ms: number) => {},
) => {
  const fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push(new Request(input, init));
    const response = responses.shift() ?? new Response(null, { status: 204 });
    if (response instanceof Error) throw response;
    return response;
  };
  return {
    client: makeCoordinatorClient({
      coordinatorUrl: "https://coordinator.test",
      workerId: "worker-1",
      workerToken: "token",
      fetch,
      sleep,
    }),
    seen,
  };
};

const clientServiceOf = (layer: Layer.Layer<CoordinatorClient>): Promise<CoordinatorClient["Service"]> =>
  Effect.runPromise(Effect.provide(Effect.service(CoordinatorClient), layer));

const storeServiceOf = (layer: Layer.Layer<JobStore>): Promise<JobStore["Service"]> =>
  Effect.runPromise(Effect.provide(Effect.service(JobStore), layer));

test("empty claims do not sleep in the client", async () => {
  const { client } = clientFor([new Response(null, { status: 204 })]);
  const service = await clientServiceOf(client);
  expect(await run(service.claim())).toBeUndefined();
});

test("completion retries only network and transient responses, four times maximum", async () => {
  const record = jobRecordFixture();
  for (const status of [400, 401, 409]) {
    const seen: Request[] = [];
    const { client } = clientFor([new Response(null, { status })], seen);
    const service = await clientServiceOf(client);
    const result = await Effect.runPromiseExit(service.complete(record));
    expect(seen).toHaveLength(1);
    expect(result._tag).toBe(status === 409 ? "Success" : "Failure");
  }

  const seen: Request[] = [];
  const { client } = clientFor(
    [
      new Response(null, { status: 503 }),
      new Response(null, { status: 408 }),
      new Response(null, { status: 500 }),
      new Response(null, { status: 500 }),
    ],
    seen,
  );
  const service = await clientServiceOf(client);
  const result = await Effect.runPromiseExit(service.complete(record));
  expect(result._tag).toBe("Failure");
  expect(seen).toHaveLength(4);

  const networkSeen: Request[] = [];
  const network = clientFor(
    [
      new Error("offline"),
      new Error("offline"),
      new Error("offline"),
      new Response(null, { status: 204 }),
    ],
    networkSeen,
  );
  const networkService = await clientServiceOf(network.client);
  await run(networkService.complete(record));
  expect(networkSeen).toHaveLength(4);
});

test("a stale completion retires a durable terminal record after restart", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-recovery-"));
  try {
    const initialStoreLayer = makeFileJobStore(root);
    const initialStore = await storeServiceOf(initialStoreLayer);
    const accepted = await run(initialStore.create(request));
    await run(initialStore.put({
      ...accepted.record,
      status: "failed",
      error: "crashed",
      updatedAt: "now",
    }));

    const freshStoreLayer = makeFileJobStore(root);
    const clientLayer = clientFor([new Response(null, { status: 409 })]).client;
    const freshStore = await storeServiceOf(freshStoreLayer);
    const client = await clientServiceOf(clientLayer);
    await run(drainUnreported({ client, jobs: freshStore }));

    const persisted = await run(freshStore.get(`${request.jobId}:1:1`));
    expect(persisted?.reportedAt).toBeDefined();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a terminal duplicate assignment is re-reported without invoking Eve", async () => {
  let eveCalls = 0;
  const terminal = jobRecordFixture({
    request: { ...request, jobId: "duplicate" },
    executionId: "duplicate:1:1",
  });
  const jobs: JobStore["Service"] = {
    create: () => Effect.succeed({ record: terminal, created: false }),
    get: () => Effect.succeed(terminal),
    unreportedTerminal: () => Effect.succeed([]),
    put: () => Effect.succeed(undefined),
    advanceCursor: () => Effect.succeed(undefined),
  };
  const client: CoordinatorClient["Service"] = {
    claim: () => Effect.succeed(assignment),
    heartbeat: () => Effect.succeed(undefined),
    complete: () => Effect.succeed(undefined),
  };
  const eve: EveExecution["Service"] = {
    run: () => {
      eveCalls += 1;
      return Effect.fail(new EveFailure("Eve must not run for a terminal duplicate"));
    },
    cancel: () => Effect.fail(new EveFailure("Eve must not cancel for a terminal duplicate")),
  };

  expect(await run(runOnce({ client, jobs, eve }))).toBe(true);
  expect(await run(runOnce({ client, jobs, eve }))).toBe(true);
  expect(eveCalls).toBe(0);
});

/**
 * Eve now owns repository execution, so a worker without it must fail the
 * attempt and report it rather than silently succeeding with an empty result.
 */
test("a claim without Eve execution fails the job and reports it", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-no-eve-"));
  try {
    const storeLayer = makeFileJobStore(root);
    const jobs = await storeServiceOf(storeLayer);
    const completions: JobRecord[] = [];
    const client: CoordinatorClient["Service"] = {
      claim: () => Effect.succeed(assignment),
      heartbeat: () => Effect.succeed(undefined),
      complete: (record) => {
        completions.push(record);
        return Effect.succeed(undefined);
      },
    };

    expect(await run(runOnce({ client, jobs }))).toBe(true);
    expect(completions).toHaveLength(1);
    expect(completions[0]?.status).toBe("failed");
    expect(completions[0]?.error).toContain("Eve execution is required");

    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.status).toBe("failed");
    expect(persisted?.reportedAt).toBeDefined();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/**
 * F2. An Eve failure is an Effect failure, not a thrown exception, so the
 * generator's old `try`/`catch` never observed it: the record stayed `running`,
 * nothing was reported, and `unreportedTerminal` could not find it afterwards
 * either. The failure has to reach disk as a terminal record and be reported.
 */
test("an Eve failure persists a terminal failed record and reports it", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-fail-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const completions: JobRecord[] = [];
    const client: CoordinatorClient["Service"] = {
      claim: () => Effect.succeed(assignment),
      heartbeat: () => Effect.succeed(undefined),
      complete: (record) => {
        completions.push(record);
        return Effect.succeed(undefined);
      },
    };
    const eve: EveExecution["Service"] = {
      run: () => Effect.fail(new EveFailure("Eve turn did not complete")),
      cancel: () => Effect.succeed(undefined),
    };

    expect(await run(runOnce({ client, jobs, eve }))).toBe(true);

    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.status).toBe("failed");
    expect(persisted?.error).toContain("Eve turn did not complete");
    expect(persisted?.updatedAt).toBeDefined();
    expect(persisted?.reportedAt).toBeDefined();

    expect(completions).toHaveLength(1);
    expect(completions[0]?.status).toBe("failed");

    // Having been reported, the record must not be re-reported on the next drain.
    expect(await run(jobs.unreportedTerminal())).toHaveLength(0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/**
 * F2, continued. The heartbeat runs on a forked fiber; if the failure path
 * skips the interrupt, the fiber outlives the attempt and keeps renewing a
 * lease for work that already stopped.
 */
test("the heartbeat stops after an Eve failure", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-heartbeat-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    let beats = 0;
    // deadlineSeconds 30 / 3 gives a 10s interval, so shrink it for the test.
    const quick: ClaimAssignment = {
      ...assignment,
      job: { ...assignment.job, policy: { ...assignment.job.policy, deadlineSeconds: 1 } },
    };
    const client: CoordinatorClient["Service"] = {
      claim: () => Effect.succeed(quick),
      heartbeat: () => Effect.sync(() => { beats += 1; }),
      complete: () => Effect.succeed(undefined),
    };
    const eve: EveExecution["Service"] = {
      // Give the interval a chance to elapse at least once before failing.
      run: () => Effect.sleep("400 millis").pipe(Effect.andThen(Effect.fail(new EveFailure("Eve step.failed")))),
      cancel: () => Effect.succeed(undefined),
    };

    expect(await run(runOnce({ client, jobs, eve }))).toBe(true);

    const afterRun = beats;
    await Bun.sleep(400);
    expect(beats).toBe(afterRun);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/**
 * F2, continued. Interruption is not failure. A run cut short by shutdown must
 * leave the record `running` so a restart can resume it from its Eve cursor,
 * and must not report a fabricated terminal outcome to the coordinator.
 */
test("an interrupted run leaves the record running and reports nothing", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-interrupt-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const completions: JobRecord[] = [];
    const client: CoordinatorClient["Service"] = {
      claim: () => Effect.succeed(assignment),
      heartbeat: () => Effect.succeed(undefined),
      complete: (record) => {
        completions.push(record);
        return Effect.succeed(undefined);
      },
    };
    const eve: EveExecution["Service"] = {
      run: () => Effect.never,
      cancel: () => Effect.succeed(undefined),
    };

    const exit = await Effect.runPromiseExit(
      runOnce({ client, jobs, eve }).pipe(Effect.timeout("150 millis")),
    );
    expect(exit._tag).toBe("Failure");

    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.status).toBe("running");
    expect(persisted?.reportedAt).toBeUndefined();
    expect(completions).toHaveLength(0);
    // Nothing terminal exists, so a restart drain has nothing to retire.
    expect(await run(jobs.unreportedTerminal())).toHaveLength(0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/**
 * A non-empty required-tests list is what the coordinator grades the attempt
 * on. `EveExecution.run` succeeds only on a published receipt, which the
 * GitHub handler writes only after every required test exited 0 and the draft
 * PR was published, so success maps every declared test to exit 0; a failure
 * must not report passing tests.
 */
test("required tests map to exit-0 results only on success", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-tests-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const tests = [
      { name: "unit", command: "bun test" },
      { name: "lint", command: "bun run lint" },
    ] as const;
    const withTests: ClaimAssignment = {
      ...assignment,
      job: { ...assignment.job, policy: { ...assignment.job.policy, tests } },
    };
    const completions: JobRecord[] = [];
    const client: CoordinatorClient["Service"] = {
      claim: () => Effect.succeed(withTests),
      heartbeat: () => Effect.succeed(undefined),
      complete: (record) => {
        completions.push(record);
        return Effect.succeed(undefined);
      },
    };
    const eve: EveExecution["Service"] = {
      run: () => Effect.succeed({ summary: "patched", sessionId: "session-1" }),
      cancel: () => Effect.succeed(undefined),
    };

    expect(await run(runOnce({ client, jobs, eve }))).toBe(true);
    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.status).toBe("succeeded");
    expect(persisted?.result?.tests).toHaveLength(2);
    expect(persisted?.result?.tests.map((test) => test.exitCode)).toEqual([0, 0]);
    expect(persisted?.result?.tests.map((test) => test.name)).toEqual(["unit", "lint"]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/**
 * The published receipt carries the pull request, and it is the only place the
 * worker learns of it: Eve owns the GitHub writes and the worker makes no
 * GitHub calls of its own. The record must therefore keep it so `completionFor`
 * forwards it to celld.
 */
test("a published Eve result forwards the pull request to celld", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-pr-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const seen: Request[] = [];
    const client = await clientServiceOf(clientFor([
      new Response(null, { status: 204 }),
      new Response(null, { status: 204 }),
      new Response(null, { status: 204 }),
    ], seen).client);
    const eve: EveExecution["Service"] = {
      run: () => Effect.succeed({
        summary: "patched",
        sessionId: "session-1",
        branch: "sigiro/eve-abc",
        commitSha: "c".repeat(40),
        changedPaths: ["src/a.ts"],
        pullRequestNumber: 3,
        pullRequestUrl: "https://example.test/pr/3",
      }),
      cancel: () => Effect.succeed(undefined),
    };
    const claiming: CoordinatorClient["Service"] = { ...client, claim: () => Effect.succeed(assignment) };

    expect(await run(runOnce({ client: claiming, jobs, eve }))).toBe(true);

    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.status).toBe("succeeded");
    expect(persisted?.result?.pullRequestUrl).toBe("https://example.test/pr/3");
    expect(persisted?.result?.pullRequestNumber).toBe(3);
    expect(persisted?.result?.changedPaths).toEqual(["src/a.ts"]);

    const completion = seen.find((request) => request.url.includes("/complete"));
    expect(completion).toBeDefined();
    const body = await completion?.json() as { result?: { pullRequestUrl?: string; summary?: string } };
    expect(body.result?.pullRequestUrl).toBe("https://example.test/pr/3");
    // The summary stays the model's message rather than being replaced by the PR.
    expect(body.result?.summary).toBe("patched");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a failed attempt reports no passing tests", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-eve-tests-failed-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const withTests: ClaimAssignment = {
      ...assignment,
      job: {
        ...assignment.job,
        policy: { ...assignment.job.policy, tests: [{ name: "unit", command: "bun test" }] },
      },
    };
    const client: CoordinatorClient["Service"] = {
      claim: () => Effect.succeed(withTests),
      heartbeat: () => Effect.succeed(undefined),
      complete: () => Effect.succeed(undefined),
    };
    const eve: EveExecution["Service"] = {
      run: () => Effect.fail(new EveFailure("Eve turn.failed")),
      cancel: () => Effect.succeed(undefined),
    };

    expect(await run(runOnce({ client, jobs, eve }))).toBe(true);
    const persisted = await run(jobs.get("job-1:1:1"));
    expect(persisted?.status).toBe("failed");
    expect(persisted?.result).toBeUndefined();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/**
 * F6. `saveCursor` used to read the whole record, then write it back wholesale.
 * A terminal status written between that read and its write was silently
 * reverted to `running`, losing the result. `advanceCursor` merges only the
 * cursor and refuses to touch a record that already reached a terminal state.
 */
test("a stale cursor write does not clobber a concurrent terminal write", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-cursor-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const accepted = await run(jobs.create(request));
    await run(jobs.put({ ...accepted.record, status: "running", eveStreamIndex: 4, updatedAt: "t1" }));

    // The terminal write lands first, then a cursor write from the still-draining
    // stream loop arrives late with a higher index.
    await run(jobs.put({
      ...accepted.record,
      status: "succeeded",
      eveStreamIndex: 4,
      result: { providerOutput: "done", tests: [] },
      updatedAt: "t2",
    }));
    await run(jobs.advanceCursor(accepted.record.executionId, 9));

    const persisted = await run(jobs.get(accepted.record.executionId));
    expect(persisted?.status).toBe("succeeded");
    expect(persisted?.result?.providerOutput).toBe("done");
    expect(persisted?.eveStreamIndex).toBe(4);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("advanceCursor moves a live cursor forward and never backwards", async () => {
  const root = await mkdtemp(join(tmpdir(), "sigiro-cursor-monotonic-"));
  try {
    const jobs = await storeServiceOf(makeFileJobStore(root));
    const accepted = await run(jobs.create(request));
    await run(jobs.put({ ...accepted.record, status: "running", eveStreamIndex: 3, updatedAt: "t1" }));

    await run(jobs.advanceCursor(accepted.record.executionId, 7));
    expect((await run(jobs.get(accepted.record.executionId)))?.eveStreamIndex).toBe(7);

    await run(jobs.advanceCursor(accepted.record.executionId, 2));
    expect((await run(jobs.get(accepted.record.executionId)))?.eveStreamIndex).toBe(7);

    // An unknown execution is a no-op rather than a failure.
    await run(jobs.advanceCursor("missing:1:1", 5));
    expect(await run(jobs.get("missing:1:1"))).toBeUndefined();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
