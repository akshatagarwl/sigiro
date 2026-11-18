import { z } from "zod";

const identity = {
  executionId: z.string().min(1),
  headSha: z.string().min(1),
  sessionId: z.string().min(1),
  updatedAt: z.string().min(1),
};

export const PublishedTests = z.array(z.object({
  name: z.string().min(1),
  command: z.string().min(1),
  exitCode: z.literal(0),
})).min(1);

export const PublishedReceipt = z.object({
  ...identity,
  status: z.literal("published"),
  branch: z.string().min(1),
  commitSha: z.string().min(1),
  changedPaths: z.array(z.string().min(1)).min(1),
  pullRequestNumber: z.int(),
  pullRequestUrl: z.string().min(1),
  tests: PublishedTests,
}).strict();

export const FailedReceipt = z.object({
  ...identity,
  status: z.literal("failed"),
  error: z.string().min(1),
}).strict();

export const PublicationReceipt = z.discriminatedUnion("status", [PublishedReceipt, FailedReceipt]);
export type PublicationReceipt = z.infer<typeof PublicationReceipt>;
