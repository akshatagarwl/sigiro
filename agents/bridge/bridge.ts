import { Database } from "bun:sqlite";
import { createHash } from "node:crypto";
import { mkdir, realpath } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { DateTime, Effect, Schema } from "effect";
import { JobSpec } from "../src/contracts.ts";
import { celldPayloadOf } from "../executor/types.ts";
import { parseCelldJobBody } from "../agent/lib/celld-dispatch.ts";
import { verifiedEvidence } from "../agent/lib/workspace.ts";
import type { BridgeClient } from "./client.ts";
import { BridgeError } from "./errors.ts";
import { loadBridgeRecords, saveBridgeRecord, writeAtomicFile } from "./store.ts";
import { BridgeEvidence, BridgeRecord, type Anomaly, type BridgePolicy } from "./types.ts";

const BUCKET_US = 300 * 1_000_000;
const hash = (value: string | Uint8Array): string => createHash("sha256").update(value).digest("hex");
const fail = (operation: string, detail: string) => new BridgeError({ operation, detail, retryable: false });
const io = <A>(operation: string, action: () => Promise<A>) => Effect.tryPromise({
  try: action,
  catch: (cause) => new BridgeError({ operation, detail: cause instanceof Error ? cause.message : String(cause), retryable: !Schema.isSchemaError(cause) }),
// Native fs promises cannot be aborted. Finish them before releasing the process lock.
}).pipe(Effect.uninterruptible);
const decode = <S extends Schema.Top & { readonly DecodingServices: never }>(schema: S, value: unknown, operation: string) =>
  Schema.decodeUnknownEffect(schema)(value).pipe(Effect.mapError(() => fail(operation, "data does not match the required shape")));

export interface BridgeOptions {
  readonly settings: BridgePolicy;
  readonly stateDir: string;
  readonly evidenceDir: string;
}

const Coverage = Schema.Struct({
  coverage: Schema.Struct({ has_spans: Schema.Boolean, has_logs: Schema.Boolean, has_profiles: Schema.Boolean }),
});

/** One local owner across processes. SQLite releases its OS lock on process death. */
const acquireLock = (root: string) => io("lock bridge", async () => {
  await mkdir(root, { recursive: true, mode: 0o700 });
  const db = new Database(join(root, "ownership.sqlite"));
  try { db.exec("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE"); }
  catch (cause) { db.close(); throw cause; }
  return db;
});

const prepare = Effect.fn("Bridge.prepare")(function* (record: Extract<BridgeRecord, { status: "detected" }>, options: BridgeOptions, client: BridgeClient) {
  const diagnosis = yield* client.diagnose(record.service, record.fromTs, record.toTs);
  const { coverage } = yield* decode(Coverage, diagnosis, "validate diagnosis coverage");
  for (const anomaly of record.anomalies) {
    const covered = anomaly.signal === "profile_cost" ? coverage.has_profiles
      : anomaly.signal === "log_volume" || anomaly.signal === "error_log_rate" ? coverage.has_logs : coverage.has_spans;
    if (!covered) return yield* fail("prepare evidence", `diagnosis has no telemetry for ${anomaly.signal}`);
  }
  const baseSha = yield* client.baseSha(record.repository, record.settings.baseRef);
  const capturedAt = DateTime.toEpochMillis(yield* DateTime.now);
  const evidence = yield* decode(BridgeEvidence, {
    service: record.service, from_ts: record.fromTs, to_ts: record.toTs,
    captured_at_us: capturedAt * 1_000, incident_id: record.candidateId,
    anomalies: record.anomalies, diagnosis,
  }, "validate evidence");
  const bytes = new TextEncoder().encode(`${JSON.stringify(evidence)}\n`);
  if (bytes.byteLength > 1024 * 1024) return yield* fail("prepare evidence", "snapshot exceeds Eve's 1 MiB limit");
  const evidenceSha256 = hash(bytes);
  const evidencePath = join(options.evidenceDir, `${evidenceSha256}.json`);
  const job = yield* decode(JobSpec, {
    jobId: `sigiro-${record.candidateId}`, generation: 1,
    repository: record.repository, baseRef: record.settings.baseRef, baseSha,
    incidentId: record.candidateId, evidenceUri: pathToFileURL(evidencePath).href, evidenceSha256,
    prompt: "Investigate the incident in /workspace/sigiro-evidence.json against the pinned checkout. " +
      "Use the original anomaly observations and diagnosis to establish a supported cause before editing. " +
      "Make a justified fix within the supplied policy and run every required test. " +
      "If the evidence does not support a fix, explain what is missing and stop without editing.",
    policy: record.settings.policy,
  }, "validate job");
  // Reserve worst-case JSON escaping for the dispatch schema's 512-character worker ID.
  const dispatch = celldPayloadOf({ ...job, attempt: job.policy.maxAttempts, tests: job.policy.tests, allowedPaths: job.policy.allowedPaths }, "\u0000".repeat(512));
  if (parseCelldJobBody(new TextEncoder().encode(JSON.stringify(dispatch))) === null) {
    return yield* fail("validate job", "combined job policy exceeds Eve's dispatch limits");
  }
  yield* io("write evidence", () => writeAtomicFile(evidencePath, bytes));
  const ready: BridgeRecord = { ...record, status: "ready", job, updatedAt: capturedAt };
  yield* io("save prepared job", () => saveBridgeRecord(join(options.stateDir, "records"), ready));
  return ready;
});

const submit = Effect.fn("Bridge.submit")(function* (record: Extract<BridgeRecord, { status: "ready" }>, options: BridgeOptions, client: BridgeClient) {
  // Reuse Eve's bounded local-file, directory, and digest verification on every replay.
  yield* Effect.tryPromise({
    try: () => verifiedEvidence(record.job),
    catch: (cause) => fail("verify prepared evidence", cause instanceof Error ? cause.message : String(cause)),
  }).pipe(Effect.uninterruptible);
  const remote = yield* client.submit(record.job);
  const submitted: BridgeRecord = { ...record, status: "submitted", updatedAt: DateTime.toEpochMillis(yield* DateTime.now) };
  yield* io("save submission", () => saveBridgeRecord(join(options.stateDir, "records"), submitted));
  yield* Effect.logInfo("bridge job submitted").pipe(Effect.annotateLogs({ "job.hash": hash(record.job.jobId) }));
  return { status: "submitted" as const, candidateId: record.candidateId, jobId: record.job.jobId, jobStatus: remote.status };
});

/** A successful worker job occupies the repository until its draft PR is closed. */
const occupied = Effect.fn("Bridge.occupied")(function* (record: Extract<BridgeRecord, { status: "submitted" }>, client: BridgeClient) {
  const remote = yield* client.status(record.job.jobId);
  if (remote === undefined) return yield* fail("read submitted job", "coordinator lost an acknowledged job; repository remains reserved");
  if (JSON.stringify(remote.spec) !== JSON.stringify(record.job)) return yield* fail("read submitted job", "coordinator job input changed");
  if (remote.status !== "succeeded") return remote.status === "no_fix" ? undefined : remote.status;
  const parsed = yield* Effect.try({
    try: () => new URL(remote.result?.pullRequestUrl ?? ""),
    catch: () => fail("read publication", "successful job has no valid pull request URL"),
  });
  const prefix = `/${record.repository}/pull/`;
  if (parsed.origin !== "https://github.com" || !parsed.pathname.startsWith(prefix) || !/^[1-9][0-9]*$/.test(parsed.pathname.slice(prefix.length))) {
    return yield* fail("read publication", "pull request does not belong to the configured repository");
  }
  const pr = yield* client.pullRequest(record.repository, Number(parsed.pathname.slice(prefix.length)));
  return pr.state === "open" ? "draft_pr_open" : undefined;
});

export const bridgeOnce = Effect.fn("bridgeOnce")(function* (options: BridgeOptions, client: BridgeClient) {
  yield* Effect.acquireRelease(acquireLock(options.stateDir), (db) => Effect.sync(() => db.close()));
  yield* io("create evidence directory", () => mkdir(options.evidenceDir, { recursive: true, mode: 0o700 }));
  options = { ...options, evidenceDir: yield* io("resolve evidence directory", () => realpath(options.evidenceDir)) };
  const recordsDir = join(options.stateDir, "records");
  const records = [...yield* io("load bridge records", () => loadBridgeRecords(recordsDir))];
  for (const record of records) {
    if (record.sourceId !== options.settings.sourceId || record.service !== options.settings.service || record.repository !== options.settings.repository) {
      return yield* fail("load bridge records", "state directory belongs to a different source, service, or repository");
    }
    if (record.status === "blocked") return yield* fail("resume bridge", record.lastError);
    if (record.status === "submitted") {
      const reason = yield* occupied(record, client);
      if (reason !== undefined) return { status: "waiting" as const, jobId: record.job.jobId, reason };
    }
  }
  let pending: BridgeRecord | undefined = records.find((record) => record.status === "ready" || record.status === "detected");
  const nowMs = DateTime.toEpochMillis(yield* DateTime.now);
  const nowUs = nowMs * 1_000;
  if (pending === undefined) {
    const anomalies = yield* client.anomalies(options.settings.service);
    const groups = new Map<string, Anomaly[]>();
    for (const anomaly of anomalies) {
      const key = anomaly.incident_id ?? JSON.stringify([anomaly.service_name, anomaly.operation, anomaly.signal, anomaly.ts_shift_us]);
      const members = groups.get(key) ?? [];
      members.push(anomaly); groups.set(key, members);
    }
    for (const anomalies of groups.values()) {
      const memberKeys = anomalies.map((row) => hash(JSON.stringify([options.settings.sourceId, row.service_name, row.operation, row.signal, row.ts_shift_us]))).sort();
      const handled = records.find((record) => record.memberKeys.some((key) => memberKeys.includes(key)));
      if (handled !== undefined) {
        const keys = [...new Set([...handled.memberKeys, ...memberKeys])].sort();
        if (keys.length !== handled.memberKeys.length) {
          const updated = yield* decode(BridgeRecord, { ...handled, memberKeys: keys, updatedAt: nowMs }, "validate incident members");
          yield* io("remember incident members", () => saveBridgeRecord(recordsDir, updated));
          records[records.indexOf(handled)] = updated;
        }
        continue;
      }
      const fromTs = Math.min(...anomalies.map((row) => row.ts_shift_us));
      const lastShift = Math.max(...anomalies.map((row) => row.ts_shift_us));
      if (nowUs - fromTs > options.settings.maxAgeSeconds * 1_000_000) continue;
      if (lastShift > nowUs) return yield* fail("select anomaly", "detector returned a future shift timestamp");
      const toTs = Math.max(Math.floor(nowUs / BUCKET_US) * BUCKET_US, Math.floor(lastShift / BUCKET_US) * BUCKET_US + BUCKET_US);
      pending = yield* decode(BridgeRecord, {
        status: "detected", candidateId: hash(JSON.stringify([options.settings.sourceId, options.settings.repository, memberKeys])),
        sourceId: options.settings.sourceId, service: options.settings.service, repository: options.settings.repository,
        anomalies, memberKeys, fromTs, toTs, createdAt: nowMs, updatedAt: nowMs, settings: options.settings,
      }, "validate candidate");
      yield* io("save detected incident", () => saveBridgeRecord(recordsDir, pending!));
      break;
    }
  }
  if (pending === undefined) return { status: "idle" as const };
  const record = pending;
  if (record.status !== "detected" && record.status !== "ready") return yield* fail("resume bridge", "unexpected pending state");
  if (record.status === "detected" && record.toTs > nowUs) return { status: "waiting" as const, reason: "detection_bucket_incomplete", candidateId: record.candidateId };
  const run = Effect.gen(function* () {
    if (record.status === "detected" && nowUs - record.fromTs > record.settings.maxAgeSeconds * 1_000_000) {
      return yield* fail("prepare incident", "incident exceeded its configured evidence age before preparation");
    }
    const ready = record.status === "ready" ? record : yield* prepare(record, options, client);
    return yield* submit(ready, options, client);
  });
  return yield* run.pipe(Effect.tapError((error) => Effect.gen(function* () {
    // Reload: preparation may already have frozen a job. Never roll ready back to detected.
    const latest = (yield* io("reload failed bridge", () => loadBridgeRecords(recordsDir))).find((value) => value.candidateId === record.candidateId);
    if (latest === undefined) return yield* fail("save bridge failure", "candidate record disappeared");
    const failed: BridgeRecord = error.retryable
      ? { ...latest, lastError: error.message, updatedAt: nowMs }
      : { ...latest, status: "blocked", lastError: error.message, updatedAt: nowMs };
    yield* io("save bridge failure", () => saveBridgeRecord(recordsDir, failed));
  })));
}, Effect.scoped);
