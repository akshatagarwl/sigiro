import type { SessionContext } from "eve/context";
import { PublishedReceipt, PublishedTests } from "../../src/publication.ts";
import type { WorldJob } from "./celld-dispatch.ts";
import { publishDraftPr } from "./github-draft-pr.ts";
import type { DraftPrPublishInput } from "./github-draft-pr.ts";
import { writeReceipt } from "./publication-receipts.ts";
import { CHECKOUT_PATH } from "./workspace.ts";

const quote = (value: string): string => `'${value.replaceAll("'", "'\\''")}'`;

export async function publishJob({ job, session, sandbox, github }: {
  readonly job: WorldJob;
  readonly session: SessionContext["session"];
  readonly sandbox: DraftPrPublishInput["sandbox"];
  readonly github: DraftPrPublishInput["github"];
}): Promise<void> {
  if (github.repository.fullName !== job.repository) throw new Error("Publication repository does not match the job");
  const assertHead = async () => {
    const result = await sandbox.run({ command: `git -C ${quote(CHECKOUT_PATH)} rev-parse HEAD` });
    if (result.exitCode !== 0 || result.stdout.trim() !== job.baseSha) throw new Error("Publication checkout does not match the job's pinned base SHA");
  };
  await assertHead();
  const results = [];
  for (const test of job.tests) results.push({ ...test, exitCode: (await sandbox.run({ command: `cd ${quote(CHECKOUT_PATH)} && /bin/sh -lc ${quote(test.command)}` })).exitCode });
  const failed = results.find((test) => test.exitCode !== 0);
  if (failed !== undefined) throw new Error(`Required test failed: ${failed.name} (exit ${failed.exitCode})`);
  const tests = PublishedTests.parse(results);
  await assertHead();
  const publication = await publishDraftPr({ github, sandbox, publicationId: job.executionId, allowedPaths: job.allowedPaths, tests, state: { checkoutPath: CHECKOUT_PATH, headSha: job.baseSha, baseRef: job.baseRef, defaultBranch: job.baseRef } });
  if (publication === null) throw new Error("World publication produced no changes");
  const pullRequestUrl = publication.pullRequest.htmlUrl ?? `https://github.com/${job.repository}/pull/${publication.pullRequest.number}`;
  await writeReceipt(PublishedReceipt.parse({ executionId: job.executionId, headSha: job.baseSha, sessionId: session.id, updatedAt: new Date().toISOString(), status: "published", tests, branch: publication.branch, commitSha: publication.commitSha, changedPaths: publication.changedPaths, pullRequestNumber: publication.pullRequest.number, pullRequestUrl }));
  console.info(`Draft PR: ${pullRequestUrl}`);
}
