import { beforeEach, expect, test } from "bun:test";
import coordinator, { Job, JobQueue } from "../coordinator/index.ts";
import type {
  CoordinatorEnv, DurableObjectId, DurableObjectNamespace,
  DurableObjectState, DurableObjectStorage, DurableObjectStub,
} from "../coordinator/runtime.ts";

class MemoryStorage implements DurableObjectStorage {
  readonly values = new Map<string, unknown>();
  alarm: number | null = null;
  async get<T>(k: string): Promise<T | undefined> { return this.values.get(k) as T | undefined; }
  async put<T>(k: string, v: T): Promise<void> { this.values.set(k, structuredClone(v)); }
  async deleteAlarm(): Promise<void> { this.alarm = null; }
  async getAlarm(): Promise<number | null> { return this.alarm; }
  async setAlarm(t: number | Date): Promise<void> { this.alarm = typeof t === "number" ? t : t.getTime(); }
}
class Id implements DurableObjectId { constructor(readonly name: string) {} }
class MemoryNamespace implements DurableObjectNamespace {
  readonly objects = new Map<string, { job: DurableObjectStub; storage: MemoryStorage }>();
  storageFactory: () => MemoryStorage = () => new MemoryStorage();
  constructor(private readonly env: () => CoordinatorEnv, private readonly queue = false) {}
  idFromName(name: string): DurableObjectId { return new Id(name); }
  get(id: DurableObjectId): DurableObjectStub {
    const name = (id as Id).name;
    let o = this.objects.get(name);
    if (o === undefined) {
      const storage = this.storageFactory();
      const job = this.queue ? new JobQueue({ storage } satisfies DurableObjectState, this.env()) : new Job({ storage } satisfies DurableObjectState, this.env());
      o = { storage, job };
      this.objects.set(name, o);
    }
    return { fetch: (r) => o!.job.fetch(r) };
  }
}

let env: CoordinatorEnv;
beforeEach(() => {
  const ns = new MemoryNamespace(() => env);
  const queue = new MemoryNamespace(() => env, true);
  env = {
    JOBS: ns, QUEUE: queue, WORKER_TOKEN: "t",
  };
});

/** A storage that fails, to prove a server fault is not reported as the caller's. */
class BrokenStorage extends MemoryStorage {
  override async get<T>(): Promise<T | undefined> {
    throw new Error("durable storage is unavailable");
  }
}

const post = (body: string) =>
  coordinator.fetch(new Request("https://c.test/v1/jobs", {
    method: "POST", headers: { "content-type": "application/json" }, body,
  }), env);

/**
 * BoundedJson replaced a hand-written content-length check. If it does not
 * actually bound the body, a control was lost rather than delegated, so each
 * arm here must fail for its own reason and the valid arm must still pass.
 */
test("the schema bounds the body, the syntax and the shape", async () => {
  const valid = JSON.stringify({
    jobId: "job-1", generation: 1, repository: "o/r", github: { installationId: 1, issueNumber: 1 }, baseRef: "main",
    baseSha: "0".repeat(40), incidentId: "i", evidenceUri: "u",
    evidenceSha256: "a".repeat(64), prompt: "p",
    policy: { maxAttempts: 1, deadlineSeconds: 30, allowedPaths: ["**"], tests: [] },
  });
  expect((await post(valid)).status).toBe(201);

  // Oversized: a 300 KiB body must be refused, not parsed.
  const oversized = JSON.stringify({ jobId: "job-2", pad: "x".repeat(300 * 1024) });
  expect(oversized.length).toBeGreaterThan(256 * 1024);
  expect((await post(oversized)).status).toBe(400);

  // Malformed JSON.
  expect((await post('{"jobId":"job-3",')).status).toBe(400);

  // Well-formed JSON, wrong shape: generation must be a positive int.
  const badShape = JSON.parse(valid) as Record<string, unknown>;
  badShape.jobId = "job-4";
  badShape.generation = 0;
  expect((await post(JSON.stringify(badShape))).status).toBe(400);
});

test("requires exactly one positive GitHub issue or pull request anchor", async () => {
  const base = {
    jobId: "job-anchor", generation: 1, repository: "o/r", baseRef: "main",
    baseSha: "0".repeat(40), incidentId: "i", evidenceUri: "u",
    evidenceSha256: "a".repeat(64), prompt: "p",
    policy: { maxAttempts: 1, deadlineSeconds: 30, allowedPaths: ["**"], tests: [] },
  };
  for (const [index, github] of [
    { installationId: 1, issueNumber: 1 },
    { installationId: 1, pullRequestNumber: 1 },
  ].entries()) {
    expect((await post(JSON.stringify({ ...base, github, jobId: `valid-${index}` }))).status).toBe(201);
  }
  for (const github of [
    { installationId: 1 },
    { installationId: 1, issueNumber: 1, pullRequestNumber: 1 },
    { installationId: 0, issueNumber: 1 },
    { installationId: 1, issueNumber: 0 },
    { installationId: 1, pullRequestNumber: 0 },
  ]) {
    expect((await post(JSON.stringify({ ...base, github, jobId: `bad-${JSON.stringify(github)}` }))).status).toBe(400);
  }
});

test("accepts only full lowercase Git object IDs", async () => {
  const base = {
    jobId: "job-sha", generation: 1, repository: "o/r", github: { installationId: 1, pullRequestNumber: 1 }, baseRef: "main",
    baseSha: "0".repeat(40), incidentId: "i", evidenceUri: "u",
    evidenceSha256: "a".repeat(64), prompt: "p",
    policy: { maxAttempts: 1, deadlineSeconds: 30, allowedPaths: ["**"], tests: [] },
  };
  for (const baseSha of ["abc123", "0".repeat(39), "a".repeat(40).toUpperCase(), "0".repeat(41), "refs/heads/main"]) {
    expect((await post(JSON.stringify({ ...base, jobId: `job-${baseSha.length}`, baseSha }))).status).toBe(400);
  }
  expect((await post(JSON.stringify({ ...base, baseSha: "a".repeat(64) }))).status).toBe(201);
});

test("a malformed body is 400, a storage fault is 500", async () => {
  const valid = JSON.stringify({
    jobId: "job-9", generation: 1, repository: "o/r", github: { installationId: 1, issueNumber: 1 }, baseRef: "main",
    baseSha: "0".repeat(40), incidentId: "i", evidenceUri: "u",
    evidenceSha256: "a".repeat(64), prompt: "p",
    policy: { maxAttempts: 1, deadlineSeconds: 30, allowedPaths: ["**"], tests: [] },
  });

  // Caller's fault.
  expect((await post("{not json")).status).toBe(400);

  // Ours. Reporting this as 400 told the executor to stop retrying a request
  // that would have succeeded once storage recovered.
  const broken = new MemoryNamespace(() => brokenEnv);
  broken.storageFactory = () => new BrokenStorage();
  const brokenEnv: CoordinatorEnv = {
    JOBS: broken, QUEUE: broken, WORKER_TOKEN: "t",
  };
  const response = await coordinator.fetch(new Request("https://c.test/v1/jobs", {
    method: "POST", headers: { "content-type": "application/json" }, body: valid,
  }), brokenEnv);
  expect(response.status).toBe(500);
});
