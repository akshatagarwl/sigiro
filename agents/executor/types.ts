import { Schema } from "effect";
import { PublicationReceipt } from "../src/publication.ts";
import { ClaimAssignment, CompleteJobRequest, executionIdOf, PositiveInt, Sha256Hex, TestCommand } from "../src/contracts.ts";

/** The executor's local form of the coordinator's claimed assignment. */
export const JobRequest = Schema.Struct({
  jobId: Schema.NonEmptyString,
  generation: PositiveInt,
  attempt: PositiveInt,
  repository: Schema.NonEmptyString,
  baseRef: Schema.NonEmptyString,
  baseSha: Schema.NonEmptyString,
  incidentId: Schema.NonEmptyString,
  evidenceUri: Schema.NonEmptyString,
  evidenceSha256: Sha256Hex,
  prompt: Schema.NonEmptyString,
  tests: Schema.Array(TestCommand),
  allowedPaths: Schema.Array(Schema.String),
  deadlineSeconds: PositiveInt,
  leaseToken: Schema.NonEmptyString,
});
export type JobRequest = typeof JobRequest.Type;

export const jobRequestOf = (assignment: ClaimAssignment): JobRequest => ({
  jobId: assignment.job.jobId,
  generation: assignment.job.generation,
  attempt: assignment.attempt,
  repository: assignment.job.repository,
  baseRef: assignment.job.baseRef,
  baseSha: assignment.job.baseSha,
  incidentId: assignment.job.incidentId,
  evidenceUri: assignment.job.evidenceUri,
  evidenceSha256: assignment.job.evidenceSha256,
  prompt: assignment.job.prompt,
  tests: assignment.job.policy.tests,
  allowedPaths: assignment.job.policy.allowedPaths,
  deadlineSeconds: assignment.job.policy.deadlineSeconds,
  leaseToken: assignment.leaseToken,
});

export const completionFor = (record: JobRecord): CompleteJobRequest => ({
  jobId: record.request.jobId,
  generation: record.request.generation,
  attempt: record.request.attempt,
  leaseToken: record.request.leaseToken,
  outcome: record.status === "succeeded" ? "succeeded" : "failed",
  result: {
    summary: record.receipt?.status === "published" ? `Published ${record.receipt.pullRequestUrl}` : record.error ?? "executor failed without an error",
    ...(record.receipt?.status === "published" ? { pullRequestUrl: record.receipt.pullRequestUrl } : {}),
    tests: record.receipt?.status === "published" ? record.receipt.tests.map(test => ({ ...test, status: "passed" as const })) : [],
  },
});

/** One definition: the schema the HTTP surface needs, and the type derived from it. */
export const JobStatus = Schema.Literals(["accepted", "running", "succeeded", "failed"]);
export type JobStatus = typeof JobStatus.Type;

/** The shape the job store persists, validated on the way back off disk. */
export const JobRecord = Schema.Struct({
  executionId: Schema.NonEmptyString,
  requestHash: Schema.NonEmptyString,
  request: JobRequest,
  status: JobStatus,
  acceptedAt: Schema.String,
  updatedAt: Schema.String,
  receipt: Schema.optionalKey(PublicationReceipt),
  eveSessionId: Schema.optionalKey(Schema.NonEmptyString),
  error: Schema.optionalKey(Schema.String),
  /** Set only after the coordinator has accepted (or retired) the result. */
  reportedAt: Schema.optionalKey(Schema.String),
});
export type JobRecord = typeof JobRecord.Type;

export const executionId = (request: JobRequest): string =>
  executionIdOf(request.jobId, request.generation, request.attempt);

/** The same dispatch shape is bounded by the bridge and sent by the executor. */
export const celldPayloadOf = (request: Omit<JobRequest, "leaseToken" | "deadlineSeconds">, workerId: string) => ({
  executionId: executionIdOf(request.jobId, request.generation, request.attempt), workerId,
  repository: request.repository, baseRef: request.baseRef, baseSha: request.baseSha,
  prompt: request.prompt, tests: request.tests, allowedPaths: request.allowedPaths,
  incidentId: request.incidentId, evidenceUri: request.evidenceUri, evidenceSha256: request.evidenceSha256,
});
