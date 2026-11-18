import { Schema } from "effect";
import { ClaimAssignment, CompleteJobRequest, executionIdOf, GithubAnchor, PositiveInt, TestCommand } from "../src/contracts.ts";

/** The executor's local form of the coordinator's claimed assignment. */
export const JobRequest = Schema.Struct({
  jobId: Schema.NonEmptyString,
  generation: PositiveInt,
  attempt: PositiveInt,
  repository: Schema.NonEmptyString,
  github: GithubAnchor,
  baseRef: Schema.NonEmptyString,
  baseSha: Schema.NonEmptyString,
  prompt: Schema.NonEmptyString,
  tests: Schema.Array(TestCommand),
  allowedPaths: Schema.Array(Schema.String),
  deadlineSeconds: PositiveInt,
  leaseToken: Schema.NonEmptyString,
});
export type JobRequest = typeof JobRequest.Type;

export const jobRequestOf = (assignment: typeof ClaimAssignment.Type): JobRequest => ({
  jobId: assignment.job.jobId,
  generation: assignment.job.generation,
  attempt: assignment.attempt,
  repository: assignment.job.repository,
  github: assignment.job.github,
  baseRef: assignment.job.baseRef,
  baseSha: assignment.job.baseSha,
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
    summary: record.result?.providerOutput.slice(0, 4_000) ?? record.error ?? "executor failed without an error",
    ...(record.result?.pullRequestUrl === undefined ? {} : { pullRequestUrl: record.result.pullRequestUrl }),
    ...(record.result?.patchSha256 === undefined ? {} : { patchSha256: record.result.patchSha256 }),
    tests: (record.result?.tests ?? []).map(({ name, command, exitCode }) => ({
      name, command, exitCode, status: exitCode === 0 ? "passed" as const : "failed" as const,
    })),
  },
});

/** One definition: the schema the HTTP surface needs, and the type derived from it. */
export const JobStatus = Schema.Literals(["accepted", "running", "succeeded", "failed"]);
export type JobStatus = typeof JobStatus.Type;

export const TestResult = Schema.Struct({
  name: Schema.String,
  command: Schema.String,
  exitCode: Schema.Number,
  stdout: Schema.String,
  stderr: Schema.String,
});
export type TestResult = typeof TestResult.Type;

const JobResult = Schema.Struct({
  patch: Schema.optionalKey(Schema.String),
  patchSha256: Schema.optionalKey(Schema.String),
  changedPaths: Schema.optionalKey(Schema.Array(Schema.String)),
  providerOutput: Schema.String,
  tests: Schema.Array(TestResult),
  pullRequestUrl: Schema.optionalKey(Schema.NonEmptyString),
  pullRequestNumber: Schema.optionalKey(Schema.Int),
});

/** The shape the job store persists, validated on the way back off disk. */
export const JobRecord = Schema.Struct({
  executionId: Schema.NonEmptyString,
  requestHash: Schema.NonEmptyString,
  request: JobRequest,
  status: JobStatus,
  acceptedAt: Schema.String,
  updatedAt: Schema.String,
  result: Schema.optionalKey(JobResult),
  eveSessionId: Schema.optionalKey(Schema.NonEmptyString),
  eveStreamIndex: Schema.optionalKey(Schema.Int),
  error: Schema.optionalKey(Schema.String),
  /** Set only after the coordinator has accepted (or retired) the result. */
  reportedAt: Schema.optionalKey(Schema.String),
});
export type JobRecord = typeof JobRecord.Type;

export const executionId = (request: JobRequest): string =>
  executionIdOf(request.jobId, request.generation, request.attempt);
