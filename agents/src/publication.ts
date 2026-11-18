import { Schema } from "effect";

const identity = {
  executionId: Schema.NonEmptyString,
  headSha: Schema.String,
  sessionId: Schema.String,
  updatedAt: Schema.String,
};

export const PublishedTests = Schema.NonEmptyArray(Schema.Struct({
  name: Schema.NonEmptyString,
  command: Schema.NonEmptyString,
  exitCode: Schema.Literal(0),
}));

export const PublishedReceipt = Schema.Struct({
  ...identity,
  status: Schema.Literal("published"),
  branch: Schema.NonEmptyString,
  commitSha: Schema.NonEmptyString,
  changedPaths: Schema.NonEmptyArray(Schema.NonEmptyString),
  pullRequestNumber: Schema.Int,
  pullRequestUrl: Schema.NonEmptyString,
  tests: PublishedTests,
});

/** One wire and disk contract shared by Eve and the executor. */
export const PublicationReceipt = Schema.Union([PublishedReceipt, Schema.Struct({
  ...identity,
  status: Schema.Literal("failed"),
  error: Schema.NonEmptyString,
})]);
export type PublicationReceipt = typeof PublicationReceipt.Type;
