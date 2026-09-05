import { Schema } from "effect";

export const PositiveInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(1));
export type PositiveInt = typeof PositiveInt.Type;

export const CountInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));
export type CountInt = typeof CountInt.Type;

export const Sha256Hex = Schema.String.check(Schema.isPattern(/^[0-9a-f]{64}$/));
export type Sha256Hex = typeof Sha256Hex.Type;

export const GitObjectId = Schema.String.check(Schema.isPattern(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/));
export type GitObjectId = typeof GitObjectId.Type;

export const ClaimId = Schema.NonEmptyString;
export type ClaimId = typeof ClaimId.Type;

export const JobStatus = Schema.Literals([
  "queued",
  "accepted",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "no_fix",
  "needs_review",
]);
export type JobStatus = typeof JobStatus.Type;

export const TerminalJobStatus = Schema.Literals([
  "succeeded",
  "failed",
  "cancelled",
  "no_fix",
  "needs_review",
]);
export type TerminalJobStatus = typeof TerminalJobStatus.Type;

export const isTerminalJobStatus = Schema.is(TerminalJobStatus);

export const TestCommand = Schema.Struct({
  name: Schema.NonEmptyString,
  command: Schema.NonEmptyString,
});
export type TestCommand = typeof TestCommand.Type;

export const JobPolicy = Schema.Struct({
  maxAttempts: PositiveInt,
  deadlineSeconds: PositiveInt,
  allowedPaths: Schema.Array(Schema.NonEmptyString),
  tests: Schema.NonEmptyArray(TestCommand),
});
export type JobPolicy = typeof JobPolicy.Type;

export const JobSpec = Schema.Struct({
  jobId: Schema.NonEmptyString,
  generation: PositiveInt,
  repository: Schema.NonEmptyString,
  baseRef: Schema.NonEmptyString,
  baseSha: GitObjectId,
  incidentId: Schema.NonEmptyString,
  evidenceUri: Schema.NonEmptyString,
  evidenceSha256: Sha256Hex,
  prompt: Schema.NonEmptyString,
  policy: JobPolicy,
});
export type JobSpec = typeof JobSpec.Type;

export const CreateJobRequest = JobSpec;
export type CreateJobRequest = typeof CreateJobRequest.Type;

export const JobIdentity = Schema.Struct({ jobId: Schema.NonEmptyString });
export type JobIdentity = typeof JobIdentity.Type;

export const AttemptRef = Schema.Struct({
  jobId: Schema.NonEmptyString,
  generation: PositiveInt,
  attempt: PositiveInt,
});
export type AttemptRef = typeof AttemptRef.Type;

export const WorkerClaimRequest = Schema.Struct({ workerId: Schema.NonEmptyString, claimId: ClaimId });
export type WorkerClaimRequest = typeof WorkerClaimRequest.Type;

export const LeaseRef = Schema.Struct({
  ...AttemptRef.fields,
  leaseToken: Schema.NonEmptyString,
});
export type LeaseRef = typeof LeaseRef.Type;

export const CompleteJobRequest = Schema.Struct({
  ...LeaseRef.fields,
  outcome: Schema.Literals(["succeeded", "failed", "no_fix", "needs_review"]),
  result: Schema.Struct({
    summary: Schema.String,
    artifactUri: Schema.optionalKey(Schema.NonEmptyString),
    patchSha256: Schema.optionalKey(Sha256Hex),
    pullRequestUrl: Schema.optionalKey(Schema.NonEmptyString),
    tests: Schema.Array(Schema.Struct({
      name: Schema.NonEmptyString,
      command: Schema.NonEmptyString,
      exitCode: Schema.Int,
      status: Schema.Literals(["passed", "failed", "skipped"]),
    })),
  }),
});
export type CompleteJobRequest = typeof CompleteJobRequest.Type;

export const JobClaim = Schema.Struct({
  workerId: Schema.NonEmptyString,
  claimId: ClaimId,
  leaseToken: Schema.NonEmptyString,
  leaseExpiresAt: Schema.NonEmptyString,
});
export type JobClaim = typeof JobClaim.Type;

export const StoredJob = Schema.Struct({
  spec: JobSpec,
  specSha256: Sha256Hex,
  status: JobStatus,
  attempt: CountInt,
  createdAt: Schema.NonEmptyString,
  updatedAt: Schema.NonEmptyString,
  claim: Schema.optionalKey(JobClaim),
  result: Schema.optionalKey(CompleteJobRequest.fields.result),
  lastError: Schema.optionalKey(Schema.String),
});
export type StoredJob = typeof StoredJob.Type;

export const PublicStoredJob = Schema.Struct({
  spec: JobSpec,
  specSha256: Sha256Hex,
  status: JobStatus,
  attempt: CountInt,
  createdAt: Schema.NonEmptyString,
  updatedAt: Schema.NonEmptyString,
  workerId: Schema.optionalKey(Schema.NonEmptyString),
  leaseExpiresAt: Schema.optionalKey(Schema.NonEmptyString),
  result: Schema.optionalKey(CompleteJobRequest.fields.result),
  lastError: Schema.optionalKey(Schema.String),
});
export type PublicStoredJob = typeof PublicStoredJob.Type;

export const ClaimAssignment = Schema.Struct({
  job: JobSpec,
  attempt: PositiveInt,
  executionId: Schema.NonEmptyString,
  leaseToken: Schema.NonEmptyString,
  leaseExpiresAt: Schema.NonEmptyString,
});
export type ClaimAssignment = typeof ClaimAssignment.Type;

export const executionIdOf = (jobId: string, generation: number, attempt: number): string =>
  `${jobId}:${generation}:${attempt}`;
