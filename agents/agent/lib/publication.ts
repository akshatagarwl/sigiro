import { Schema } from "effect";
import type { SessionContext } from "eve/context";
import { quote } from "shell-quote";
import { PublishedReceipt, PublishedTests } from "../../src/publication.ts";
import type { CelldJob } from "./celld-dispatch.ts";
import { publishDraftPr } from "./github-draft-pr.ts";
import type { DraftPrPublishInput } from "./github-draft-pr.ts";
import { writeReceipt } from "./publication-receipts.ts";
import { CHECKOUT_PATH } from "./workspace.ts";

/** Publish only the active worker's pinned job through the trusted host client. */
export async function publishJob({ job, session, sandbox, github }: {
  readonly job: CelldJob;
  readonly session: SessionContext["session"];
  readonly sandbox: DraftPrPublishInput["sandbox"];
  readonly github: DraftPrPublishInput["github"];
}): Promise<void> {
  const auth = session.auth.current;
  if (auth?.authenticator !== "celld-worker-token" ||
      auth.attributes.executionId !== job.executionId ||
      auth.attributes.expectedBaseSha !== job.baseSha ||
      auth.attributes.baseRef !== job.baseRef ||
      auth.attributes.requiredTests !== JSON.stringify(job.tests) ||
      JSON.stringify(auth.attributes.allowedPaths) !== JSON.stringify(job.allowedPaths)) {
    throw new Error("Publication requires the current celld worker's unchanged job policy");
  }
  if (github.repository.fullName !== job.repository) throw new Error("Publication repository does not match the job");
  const assertHead = async () => {
    const result = await sandbox.run({ command: `git -C ${quote([CHECKOUT_PATH])} rev-parse HEAD` });
    if (result.exitCode !== 0 || result.stdout.trim() !== job.baseSha) throw new Error("Publication checkout does not match the job's pinned base SHA");
  };
  await assertHead();
  const results = [];
  for (const test of job.tests) {
    const result = await sandbox.run({ command: `cd ${quote([CHECKOUT_PATH])} && /bin/sh -lc ${quote([test.command])}` });
    results.push({ ...test, exitCode: result.exitCode });
  }
  const failed = results.find((test) => test.exitCode !== 0);
  if (failed !== undefined) throw new Error(`Required test failed: ${failed.name} (exit ${failed.exitCode})`);
  const tests = Schema.decodeUnknownSync(PublishedTests)(results);
  await assertHead();
  const publication = await publishDraftPr({
    github, sandbox, publicationId: job.executionId, allowedPaths: job.allowedPaths, tests,
    state: { checkoutPath: CHECKOUT_PATH, headSha: job.baseSha, baseRef: job.baseRef, defaultBranch: job.baseRef },
  });
  if (publication === null) throw new Error("Celld publication produced no changes");
  const pullRequestUrl = publication.pullRequest.htmlUrl ?? `https://github.com/${job.repository}/pull/${publication.pullRequest.number}`;
  await writeReceipt(Schema.decodeUnknownSync(PublishedReceipt)({
    executionId: job.executionId, headSha: job.baseSha, sessionId: session.id,
    updatedAt: new Date().toISOString(), status: "published", tests,
    branch: publication.branch, commitSha: publication.commitSha,
    changedPaths: publication.changedPaths, pullRequestNumber: publication.pullRequest.number, pullRequestUrl,
  }));
  console.info(`Draft PR: ${pullRequestUrl}`);
}
