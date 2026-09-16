import { Schema } from "effect";
import { JobPolicy, JobSpec, PositiveInt, Sha256Hex } from "../src/contracts.ts";

const ShortString = Schema.NonEmptyString.check(Schema.isMaxLength(512));
const Repository = ShortString.check(Schema.isPattern(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/));
const BaseRef = ShortString.check(
  Schema.isPattern(/^[A-Za-z0-9_][A-Za-z0-9._/-]*$/),
  Schema.makeFilter((value: string) => !value.startsWith("refs/") && !value.includes("..") &&
    value.split("/").every((part) => part.length > 0 && !part.startsWith(".") && !part.endsWith(".") && !part.endsWith(".lock")) ||
    "baseRef must be a short branch name suitable for git clone --branch"),
);
// Match Eve's dispatch limits before an incident can be reserved for execution.
const ExecutionPolicy = Schema.Struct({
  ...JobPolicy.fields,
  allowedPaths: Schema.NonEmptyArray(ShortString).check(Schema.isMaxLength(1_000)),
  tests: Schema.NonEmptyArray(Schema.Struct({
    name: ShortString,
    command: Schema.NonEmptyString.check(Schema.isMaxLength(8_192)),
  })).check(Schema.isMaxLength(100)),
});

export const BridgePolicy = Schema.Struct({
  service: ShortString,
  sourceId: ShortString,
  repository: Repository,
  baseRef: BaseRef,
  policy: ExecutionPolicy,
  maxAgeSeconds: PositiveInt.check(Schema.isLessThanOrEqualTo(7 * 24 * 60 * 60)),
});
export type BridgePolicy = typeof BridgePolicy.Type;
const equalPolicy = Schema.toEquivalence(JobPolicy);

export const Anomaly = Schema.Struct({
  detected_at: Schema.NonEmptyString,
  service_name: Schema.NonEmptyString,
  operation: Schema.NonEmptyString,
  signal: Schema.Literals(["error_rate", "latency_p95", "log_volume", "error_log_rate", "profile_cost"]),
  kind: Schema.Literal("changepoint"),
  direction: Schema.Literal("worsening"),
  ts_shift: Schema.NonEmptyString,
  ts_shift_us: PositiveInt,
  before: Schema.Finite,
  after: Schema.Finite,
  incident_id: Schema.NullOr(Schema.NonEmptyString),
  drill_down_sql: Schema.optionalKey(Schema.NullOr(Schema.String)),
});
export type Anomaly = typeof Anomaly.Type;

export const AnomaliesResponse = Schema.Struct({
  anomalies: Schema.Array(Anomaly),
  truncated: Schema.Boolean,
});
export type AnomaliesResponse = typeof AnomaliesResponse.Type;

export const JsonObject = Schema.JsonObject;
export type JsonObject = typeof JsonObject.Type;

export const BridgeEvidence = Schema.Struct({
  service: Schema.NonEmptyString,
  from_ts: PositiveInt,
  to_ts: PositiveInt,
  captured_at_us: PositiveInt,
  incident_id: Schema.NonEmptyString,
  anomalies: Schema.NonEmptyArray(Anomaly),
  diagnosis: JsonObject,
});
export type BridgeEvidence = typeof BridgeEvidence.Type;

const recordFields = {
  candidateId: Sha256Hex,
  sourceId: Schema.NonEmptyString,
  service: Schema.NonEmptyString,
  repository: Schema.NonEmptyString,
  anomalies: Schema.NonEmptyArray(Anomaly),
  memberKeys: Schema.NonEmptyArray(Schema.NonEmptyString),
  fromTs: PositiveInt,
  toTs: PositiveInt,
  // Epoch milliseconds; diagnosis bounds above use epoch microseconds.
  createdAt: PositiveInt,
  updatedAt: PositiveInt,
  settings: BridgePolicy,
  lastError: Schema.optionalKey(Schema.NonEmptyString),
};

export const BridgeRecord = Schema.Union([
  Schema.Struct({ ...recordFields, status: Schema.Literal("detected") }),
  Schema.Struct({ ...recordFields, status: Schema.Literal("ready"), job: JobSpec }),
  Schema.Struct({ ...recordFields, status: Schema.Literal("submitted"), job: JobSpec }),
  Schema.Struct({ ...recordFields, status: Schema.Literal("blocked"), lastError: Schema.NonEmptyString, job: Schema.optionalKey(JobSpec) }),
]).check(Schema.makeFilter((record) => {
  if (record.sourceId !== record.settings.sourceId || record.service !== record.settings.service ||
    record.repository !== record.settings.repository) return "record identity differs from its frozen settings";
  if (record.fromTs >= record.toTs || Math.min(...record.anomalies.map((row) => row.ts_shift_us)) !== record.fromTs ||
    record.anomalies.some((row) => row.service_name !== record.service || row.ts_shift_us < record.fromTs || row.ts_shift_us >= record.toTs)) {
    return "record anomalies do not match the service and diagnosis window";
  }
  const job = "job" in record ? record.job : undefined;
  if (job !== undefined && (job.jobId !== `sigiro-${record.candidateId}` || job.incidentId !== record.candidateId ||
    job.generation !== 1 || job.repository !== record.repository || job.baseRef !== record.settings.baseRef ||
    !equalPolicy(job.policy, record.settings.policy))) return "job differs from the candidate or its frozen execution policy";
  return true;
}));
export type BridgeRecord = typeof BridgeRecord.Type;
