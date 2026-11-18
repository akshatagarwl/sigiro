import { Database } from "bun:sqlite";
import { DateTime, Effect, FileSystem, Path, Schema } from "effect";
import { JobSpec } from "../src/contracts.ts";
import { celldPayloadOf } from "../executor/types.ts";
import { parseCelldJobBody } from "../agent/lib/celld-dispatch.ts";
import { verifiedEvidence } from "../agent/lib/workspace.ts";
import { sha256 } from "../src/signing.ts";
import type { BridgeClient } from "./client.ts";
import { BridgeError } from "./errors.ts";
import { encodeBridgeJson, loadBridgeRecords, saveBridgeRecord, writeAtomicFile } from "./store.ts";
import { BridgeEvidence, BridgeRecord, type Anomaly, type BridgePolicy } from "./types.ts";

const BUCKET_US = 300 * 1_000_000;
const fail = (operation: string, detail: string) => new BridgeError({ operation, detail, retryable: false });
const isBridgeError = Schema.is(BridgeError);
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
const acquireLock = Effect.fn("acquireBridgeLock")(function* (root: string) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  yield* fs.makeDirectory(root, { recursive: true, mode: 0o700 }).pipe(Effect.mapError((cause) => fail("lock bridge", String(cause))));
  return yield* Effect.try({
    try: () => {
      const database = new Database(path.join(root, "ownership.sqlite"));
      try {
        database.exec("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE");
        return database;
      } catch (cause) {
        database.close();
        throw cause;
      }
    },
    catch: (cause) => fail("lock bridge", cause instanceof Error ? cause.message : String(cause)),
  });
});

const hashJson = Effect.fnUntraced(function* (value: unknown) {
  return yield* sha256(yield* encodeBridgeJson(value));
});

const prepare = Effect.fn("Bridge.prepare")(function* (record: Extract<BridgeRecord, { status: "detected" }>, options: BridgeOptions, client: BridgeClient) {
  const path = yield* Path.Path;
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
  const evidenceJson = `${yield* encodeBridgeJson(evidence)}\n`;
  const bytes = new TextEncoder().encode(evidenceJson);
  if (bytes.byteLength > 1024 * 1024) return yield* fail("prepare evidence", "snapshot exceeds Eve's 1 MiB limit");
  const evidenceSha256 = yield* sha256(evidenceJson);
  const evidencePath = path.join(options.evidenceDir, `${evidenceSha256}.json`);
  const job = yield* decode(JobSpec, {
    jobId: `sigiro-${record.candidateId}`, generation: 1,
    repository: record.repository, baseRef: record.settings.baseRef, baseSha,
    incidentId: record.candidateId,
    evidenceUri: (yield* path.toFileUrl(evidencePath).pipe(Effect.mapError((cause) => fail("prepare evidence", String(cause))))).href,
    prompt: "Investigate the incident in /workspace/sigiro-evidence.json against the pinned checkout. " +
      "Use the original anomaly observations and diagnosis to establish a supported cause before editing. " +
      "Make a justified fix within the supplied policy and run every required test. " +
      "If the evidence does not support a fix, explain what is missing and stop without editing.",
    policy: record.settings.policy,
  }, "validate job");
  // Reserve worst-case JSON escaping for the dispatch schema's 512-character worker ID.
  const dispatch = celldPayloadOf({ ...job, attempt: job.policy.maxAttempts, tests: job.policy.tests, allowedPaths: job.policy.allowedPaths }, "\u0000".repeat(512));
  if (parseCelldJobBody(new TextEncoder().encode(yield* encodeBridgeJson(dispatch))) === null) {
    return yield* fail("validate job", "combined job policy exceeds Eve's dispatch limits");
  }
  yield* writeAtomicFile(evidencePath, evidenceJson);
  const ready: BridgeRecord = { ...record, status: "ready", job, updatedAt: capturedAt };
  yield* saveBridgeRecord(path.join(options.stateDir, "records"), ready);
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
  const path = yield* Path.Path;
  yield* saveBridgeRecord(path.join(options.stateDir, "records"), submitted);
  yield* Effect.logInfo("bridge job submitted").pipe(Effect.annotateLogs({ "job.hash": yield* sha256(record.job.jobId) }));
  return { status: "submitted" as const, candidateId: record.candidateId, jobId: record.job.jobId, jobStatus: remote.status };
});

/** A successful worker job occupies the repository until its draft PR is closed. */
const occupied = Effect.fn("Bridge.occupied")(function* (record: Extract<BridgeRecord, { status: "submitted" }>, client: BridgeClient) {
  const remote = yield* client.status(record.job.jobId);
  if (remote === undefined) return yield* fail("read submitted job", "coordinator lost an acknowledged job; repository remains reserved");
  if ((yield* encodeBridgeJson(remote.spec)) !== (yield* encodeBridgeJson(record.job))) return yield* fail("read submitted job", "coordinator job input changed");
  if (remote.status !== "succeeded") return remote.status === "no_fix" ? undefined : remote.status;
  const parsed = yield* Schema.decodeUnknownEffect(Schema.URL)(remote.result?.pullRequestUrl ?? "").pipe(
    Effect.mapError(() => fail("read publication", "successful job has no valid pull request URL")),
  );
  const prefix = `/${record.repository}/pull/`;
  if (parsed.origin !== "https://github.com" || !parsed.pathname.startsWith(prefix) || !/^[1-9][0-9]*$/.test(parsed.pathname.slice(prefix.length))) {
    return yield* fail("read publication", "pull request does not belong to the configured repository");
  }
  const pr = yield* client.pullRequest(record.repository, Number(parsed.pathname.slice(prefix.length)));
  return pr.state === "open" ? "draft_pr_open" : undefined;
});

export const bridgeOnce = Effect.fn("bridgeOnce")(function* (options: BridgeOptions, client: BridgeClient) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  yield* Effect.acquireRelease(acquireLock(options.stateDir), (database) => Effect.sync(() => database.close()));
  yield* fs.makeDirectory(options.evidenceDir, { recursive: true, mode: 0o700 }).pipe(Effect.mapError((cause) => fail("create evidence directory", String(cause))));
  options = { ...options, evidenceDir: yield* fs.realPath(options.evidenceDir).pipe(Effect.mapError((cause) => fail("resolve evidence directory", String(cause)))) };
  const recordsDir = path.join(options.stateDir, "records");
  const records = [...yield* loadBridgeRecords(recordsDir)];
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
      const key = anomaly.incident_id ?? (yield* encodeBridgeJson([anomaly.service_name, anomaly.operation, anomaly.signal, anomaly.ts_shift_us]));
      const members = groups.get(key) ?? [];
      members.push(anomaly); groups.set(key, members);
    }
    for (const anomalies of groups.values()) {
      const memberKeys = yield* Effect.forEach(anomalies, (row) => hashJson([options.settings.sourceId, row.service_name, row.operation, row.signal, row.ts_shift_us]));
      memberKeys.sort();
      const handled = records.find((record) => record.memberKeys.some((key) => memberKeys.includes(key)));
      if (handled !== undefined) {
        const keys = [...new Set([...handled.memberKeys, ...memberKeys])].sort();
        if (keys.length !== handled.memberKeys.length) {
          const updated = yield* decode(BridgeRecord, { ...handled, memberKeys: keys, updatedAt: nowMs }, "validate incident members");
          yield* saveBridgeRecord(recordsDir, updated);
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
        status: "detected", candidateId: yield* hashJson([options.settings.sourceId, options.settings.repository, memberKeys]),
        sourceId: options.settings.sourceId, service: options.settings.service, repository: options.settings.repository,
        anomalies, memberKeys, fromTs, toTs, createdAt: nowMs, updatedAt: nowMs, settings: options.settings,
      }, "validate candidate");
      yield* saveBridgeRecord(recordsDir, pending!);
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
    const latest = (yield* loadBridgeRecords(recordsDir)).find((value) => value.candidateId === record.candidateId);
    if (latest === undefined) return yield* fail("save bridge failure", "candidate record disappeared");
    const bridgeError = isBridgeError(error) ? error : fail("bridge pass", String(error));
    const failed: BridgeRecord = bridgeError.retryable
      ? { ...latest, lastError: bridgeError.message, updatedAt: nowMs }
      : { ...latest, status: "blocked", lastError: bridgeError.message, updatedAt: nowMs };
    yield* saveBridgeRecord(recordsDir, failed);
  })));
}, Effect.scoped);
