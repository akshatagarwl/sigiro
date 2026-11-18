import { beforeEach, describe, expect, test } from "bun:test";
import coordinator, { Job, JobQueue } from "../coordinator/index.ts";
import type {
  CoordinatorEnv, DurableObjectId, DurableObjectNamespace, DurableObjectState,
  DurableObjectStorage, DurableObjectStub,
} from "../coordinator/runtime.ts";
import type { CompleteJobRequest, CreateJobRequest } from "../src/contracts.ts";

/**
 * The durable record as these tests need to read and age it. Declared here
 * rather than imported so a stored-shape change surfaces as a failure in the
 * tests that actually depend on the claim fields.
 */
interface StoredJob {
  attempt: number;
  claim: { leaseToken: string; leaseExpiresAt: string };
}

/** Fails the test at the read rather than letting `undefined` reach an assertion. */
const storedJob = async (storage: DurableObjectStorage): Promise<StoredJob> => {
  const stored = await storage.get<StoredJob>("job");
  if (stored === undefined) throw new Error("expected a stored job record");
  return stored;
};

class MemoryStorage implements DurableObjectStorage {
  readonly values = new Map<string, unknown>();
  alarm: number | null = null;
  failNextPut = false;
  failNextAlarm = false;
  async get<T>(key: string): Promise<T | undefined> { return this.values.get(key) as T | undefined; }
  async put<T>(key: string, value: T): Promise<void> {
    if (this.failNextPut) { this.failNextPut = false; throw new Error("storage write failed"); }
    this.values.set(key, structuredClone(value));
  }
  async deleteAlarm(): Promise<void> { this.alarm = null; }
  async getAlarm(): Promise<number | null> { return this.alarm; }
  async setAlarm(time: number | Date): Promise<void> {
    if (this.failNextAlarm) { this.failNextAlarm = false; throw new Error("alarm setup failed"); }
    this.alarm = typeof time === "number" ? time : time.getTime();
  }
}

class Id implements DurableObjectId { constructor(readonly name: string) {} }

class MemoryNamespace implements DurableObjectNamespace {
  readonly objects = new Map<string, { storage: MemoryStorage; stub: DurableObjectStub }>();
  constructor(private readonly env: () => CoordinatorEnv, private readonly queue = false) {}
  idFromName(name: string): DurableObjectId { return new Id(name); }
  get(id: DurableObjectId): DurableObjectStub {
    const name = (id as Id).name;
    let object = this.objects.get(name);
    if (object === undefined) {
      const storage = new MemoryStorage();
      const value = this.queue
        ? new JobQueue({ storage } satisfies DurableObjectState, this.env())
        : new Job({ storage } satisfies DurableObjectState, this.env());
      object = { storage, stub: { fetch: (request) => value.fetch(request) } };
      this.objects.set(name, object);
    }
    return object.stub;
  }
}

const makeSpec = (overrides: Partial<CreateJobRequest> = {}): CreateJobRequest => ({
  jobId: "job-123", generation: 1, repository: "customer/service", github: { installationId: 1, issueNumber: 42 }, baseRef: "main",
  baseSha: "0".repeat(40), incidentId: "incident-42", evidenceUri: "https://evidence",
  evidenceSha256: "a".repeat(64), prompt: "Fix it.",
  policy: { maxAttempts: 2, deadlineSeconds: 30, allowedPaths: ["src/**"], tests: [] },
  ...overrides,
});

let env: CoordinatorEnv;
let jobs: MemoryNamespace;
let queue: MemoryNamespace;
beforeEach(() => {
  jobs = new MemoryNamespace(() => env);
  queue = new MemoryNamespace(() => env, true);
  env = { JOBS: jobs, QUEUE: queue, WORKER_TOKEN: "worker-secret" };
});

const request = (url: string, method: string, body?: unknown, token?: string) => new Request(`https://c.test${url}`, {
  method,
  headers: {
    ...(body === undefined ? {} : { "content-type": "application/json" }),
    ...(token === undefined ? {} : { authorization: `Bearer ${token}` }),
  },
  ...(body === undefined ? {} : { body: typeof body === "string" ? body : JSON.stringify(body) }),
});
const create = (body: CreateJobRequest, targetEnv = env) => coordinator.fetch(request("/v1/jobs", "POST", body), targetEnv);
const claim = (workerId = "worker-1", token: string | undefined = "worker-secret") => coordinator.fetch(request("/v1/claim", "POST", { workerId }, token), env);
const heartbeat = (body: unknown, token?: string) => coordinator.fetch(request("/v1/jobs/job-123/heartbeat", "POST", body, token), env);
const complete = (body: CompleteJobRequest, token?: string) => coordinator.fetch(request(`/v1/jobs/${body.jobId}/complete`, "POST", body, token), env);
const status = (jobId = "job-123") => coordinator.fetch(request(`/v1/jobs/${jobId}`, "GET"), env);

const completion = (assignment: { jobId?: string; attempt: number; leaseToken: string }, outcome: CompleteJobRequest["outcome"] = "succeeded"): CompleteJobRequest => ({
  jobId: assignment.jobId ?? "job-123", generation: 1, attempt: assignment.attempt,
  leaseToken: assignment.leaseToken, outcome, result: { summary: outcome, tests: [] },
});

const expire = async (jobId: string) => {
  const object = jobs.objects.get(jobId)!;
  const stored = await storedJob(object.storage);
  stored.claim.leaseExpiresAt = new Date(0).toISOString();
  await object.storage.put("job", stored);
  await new Job({ storage: object.storage }, env).alarm();
};

class BrokenStorage extends MemoryStorage {
  override async get<T>(): Promise<T | undefined> { throw new Error("durable storage is unavailable"); }
}

describe("coordinator pull claims", () => {
  test("creates idempotently, rejects immutable conflicts, and deduplicates the queue", async () => {
    expect((await create(makeSpec())).status).toBe(201);
    expect((await create(makeSpec())).status).toBe(200);
    const pending = await queue.objects.get("global")!.storage.get<Array<{ jobId: string; claimId: string }>>("pending");
    expect(pending).toHaveLength(1);
    expect(pending![0]!.jobId).toBe("job-123");
    expect(pending![0]!.claimId).toMatch(/^[0-9a-f-]{36}$/);
    expect((await create(makeSpec({ prompt: "different" }))).status).toBe(409);
  });

  test("requires worker auth on claim, heartbeat, and complete", async () => {
    await create(makeSpec());
    expect((await coordinator.fetch(request("/v1/claim", "POST", { workerId: "worker-1" }), env)).status).toBe(401);
    expect((await claim("worker-1", "wrong")).status).toBe(401);
    const assignment = await (await claim("worker-1", "worker-secret")).json() as { attempt: number; leaseToken: string };
    const ref = { jobId: "job-123", generation: 1, attempt: assignment.attempt, leaseToken: assignment.leaseToken };
    expect((await heartbeat(ref)).status).toBe(401);
    expect((await heartbeat(ref, "wrong")).status).toBe(401);
    expect((await complete(completion(assignment))).status).toBe(401);
    expect((await complete(completion(assignment), "wrong")).status).toBe(401);
  });

  test("claims once, heartbeats accepted jobs, rejects stale leases, and hides lease tokens", async () => {
    await create(makeSpec());
    const assignment = await (await claim()).json() as { attempt: number; executionId: string; leaseToken: string };
    expect(assignment).toMatchObject({ attempt: 1, executionId: "job-123:1:1" });
    expect((await claim("worker-2", "worker-secret")).status).toBe(204);
    const ref = { jobId: "job-123", generation: 1, attempt: 1, leaseToken: assignment.leaseToken };
    expect((await heartbeat(ref, "worker-secret")).status).toBe(200);
    expect((await heartbeat({ ...ref, leaseToken: "stale" }, "worker-secret")).status).toBe(409);
    expect(JSON.stringify(await (await status()).json())).not.toContain(assignment.leaseToken);
    expect((await complete(completion(assignment), "worker-secret")).status).toBe(200);
    expect(jobs.objects.get("job-123")!.storage.alarm).toBeNull();
  });

  test("concurrent workers cannot claim the same job", async () => {
    await create(makeSpec());
    const responses = await Promise.all([claim("worker-a", "worker-secret"), claim("worker-b", "worker-secret")]);
    expect(responses.filter((response) => response.status === 200)).toHaveLength(1);
    expect(responses.filter((response) => response.status === 204)).toHaveLength(1);
  });

  test("acknowledges identical completion and conflicts on a different duplicate", async () => {
    await create(makeSpec());
    const assignment = await (await claim()).json() as { attempt: number; leaseToken: string };
    const body = completion(assignment);
    expect((await complete(body, "worker-secret")).status).toBe(200);
    expect((await complete(body, "worker-secret")).status).toBe(200);
    expect((await complete({ ...body, outcome: "failed" }, "worker-secret")).status).toBe(409);
    expect(JSON.stringify(await (await status()).json())).not.toContain(body.leaseToken);
  });

  test("rejects an old attempt after expiry and reclaim", async () => {
    await create(makeSpec());
    const first = await (await claim()).json() as { attempt: number; leaseToken: string };
    await expire("job-123");
    const second = await (await claim("worker-2", "worker-secret")).json() as { attempt: number; leaseToken: string };
    expect(second.attempt).toBe(2);
    expect((await complete(completion(first), "worker-secret")).status).toBe(409);
  });

  test("lease expiry requeues and max-attempt expiry ends failed", async () => {
    await create(makeSpec({ jobId: "job-retry" }));
    const first = await (await claim("worker-1", "worker-secret")).json() as { attempt: number };
    expect(first.attempt).toBe(1);
    await expire("job-retry");
    expect((await claim("worker-2", "worker-secret")).status).toBe(200);
    await expire("job-retry");
    expect((await status("job-retry")).status).toBe(200);
    expect((await (await status("job-retry")).json() as { status: string }).status).toBe("failed");
  });

  test("reports storage failures as 500", async () => {
    const broken = new MemoryNamespace(() => brokenEnv);
    const brokenEnv: CoordinatorEnv = { JOBS: broken, QUEUE: broken, WORKER_TOKEN: "worker-secret" };
    const brokenStorage = new BrokenStorage();
    const brokenJob = new Job({ storage: brokenStorage } satisfies DurableObjectState, brokenEnv);
    broken.objects.set("job-123", { storage: brokenStorage, stub: { fetch: (request) => brokenJob.fetch(request) } });
    expect((await coordinator.fetch(request("/v1/jobs/job-123", "GET"), brokenEnv)).status).toBe(500);
  });

  test("repairs a claim after queue removal fails with one lease and attempt", async () => {
    await create(makeSpec());
    const queueStorage = queue.objects.get("global")!.storage;
    queueStorage.failNextPut = true;
    expect((await claim()).status).toBe(500);
    const retry = await claim();
    expect(retry.status).toBe(200);
    const assignment = await retry.json() as { attempt: number; leaseToken: string };
    expect(assignment).toMatchObject({ attempt: 1 });
    expect((await status()).text()).resolves.not.toContain(assignment.leaseToken);
    expect(await queueStorage.get<unknown>("pending")).toEqual([]);
  });

  test("repairs an accepted claim after alarm setup fails", async () => {
    await create(makeSpec());
    const jobStorage = jobs.objects.get("job-123")!.storage;
    jobStorage.failNextAlarm = true;
    expect((await claim()).status).toBeGreaterThanOrEqual(500);
    const stored = await storedJob(jobStorage);
    const originalLeaseToken = stored.claim.leaseToken;
    const originalAttempt = stored.attempt;
    stored.claim.leaseExpiresAt = new Date(0).toISOString();
    await jobStorage.put("job", stored);
    const beforeRetry = Date.now();
    const retry = await claim("worker-2");
    expect(retry.status).toBe(200);
    const assignment = await retry.json() as { attempt: number; leaseToken: string; leaseExpiresAt: string };
    expect(assignment.attempt).toBe(originalAttempt);
    expect(assignment.leaseToken).toBe(originalLeaseToken);
    expect(Date.parse(assignment.leaseExpiresAt)).toBeGreaterThan(beforeRetry);
    const repaired = await storedJob(jobStorage);
    expect(repaired.attempt).toBe(originalAttempt);
    expect(repaired.claim.leaseToken).toBe(originalLeaseToken);
    expect(repaired.claim.leaseExpiresAt).toBe(assignment.leaseExpiresAt);
  });

  test("serializes enqueue and claim without losing the queue entry", async () => {
    const [created, claimed] = await Promise.all([create(makeSpec()), claim()]);
    expect([created.status, claimed.status].sort()).toEqual([201, 204]);
    const pending = await queue.objects.get("global")!.storage.get<Array<{ jobId: string; claimId: string }>>("pending");
    expect(pending).toHaveLength(1);
    expect((await status()).json()).resolves.toMatchObject({ status: "queued" });
  });

  test("rejects malformed stored queue state without mutation", async () => {
    await create(makeSpec());
    const queueStorage = queue.objects.get("global")!.storage;
    await queueStorage.put("pending", [{ jobId: "job-123" }]);
    expect((await claim()).status).toBe(500);
    expect(await queueStorage.get<unknown>("pending")).toEqual([{ jobId: "job-123" }]);
  });

  test("rejects expired heartbeat and completion before alarm delivery", async () => {
    await create(makeSpec());
    const assignment = await (await claim()).json() as { attempt: number; leaseToken: string };
    const jobStorage = jobs.objects.get("job-123")!.storage;
    const stored = await storedJob(jobStorage);
    stored.claim.leaseExpiresAt = new Date(0).toISOString();
    await jobStorage.put("job", stored);
    const before = await storedJob(jobStorage);
    const ref = { jobId: "job-123", generation: 1, attempt: assignment.attempt, leaseToken: assignment.leaseToken };
    expect((await heartbeat(ref, "worker-secret")).status).toBe(409);
    expect((await complete(completion(assignment), "worker-secret")).status).toBe(409);
    expect(await storedJob(jobStorage)).toEqual(before);
  });
});
