import { readFile } from "node:fs/promises";
import { githubChannel as eveGitHubChannel } from "eve/channels/github";
import { defaultGitHubAuth } from "eve/channels/github";
import type { GitHubChannelConfig } from "eve/channels/github";
import { publishDraftPr } from "../lib/github-draft-pr";
import type { DraftPrPublishInput, DraftPrPublishResult, DraftPrPublishState } from "../lib/github-draft-pr";
import { fileReceiptSink } from "../lib/publication-receipts";
import type { PublicationReceipt, ReceiptSink } from "../lib/publication-receipts";

/**
 * Structural subset of Eve's session auth context. Eve does not export
 * `SessionAuthContext` from a public entrypoint, so the fields this handler
 * reads are declared here rather than importing a channel-internal path.
 */
interface SessionPrincipal {
  readonly attributes: Readonly<Record<string, string | readonly string[]>>;
  readonly authenticator: string;
}

/** Authenticator name stamped by celld dispatch; see agent/lib/celld-dispatch.ts. */
export const CELLD_AUTHENTICATOR = "celld-worker-token";

/**
 * Eve credentials fields accept lazy resolvers, so a `_FILE` variant is read on
 * first use rather than at import time. Secrets stay out of the environment.
 */
const fromEnvOrFile = (name: string): (() => Promise<string>) | undefined => {
  const direct = process.env[name];
  if (direct !== undefined) return async () => direct;
  const path = process.env[`${name}_FILE`];
  if (path === undefined) return undefined;
  return async () => (await readFile(path, "utf8")).trim();
};

const appId = fromEnvOrFile("GITHUB_APP_ID");
const privateKey = fromEnvOrFile("GITHUB_APP_PRIVATE_KEY");
const webhookSecret = fromEnvOrFile("GITHUB_WEBHOOK_SECRET");

const credentials = {
  ...(appId === undefined ? {} : { appId }),
  ...(privateKey === undefined ? {} : { privateKey }),
  ...(webhookSecret === undefined ? {} : { webhookSecret }),
};

/**
 * An issue-anchored session executes at the default-branch tip, because Eve's
 * checkout resolves a null `headSha` to the repository default branch. A celld
 * job must therefore set `baseSha` to that tip; publishing a different SHA is
 * refused rather than silently re-checked-out.
 */
export const assertExpectedBaseSha = (expectedBaseSha: string | undefined, headSha: string | null): void => {
  if (expectedBaseSha !== undefined && headSha !== expectedBaseSha) {
    throw new Error(`GitHub base SHA mismatch: the anchor's checked-out head ${headSha ?? "null"} differs from the job's base SHA ${expectedBaseSha}`);
  }
};

/**
 * Enforced only when the anchor resolved a base ref. An issue-anchored session
 * leaves `state.baseRef` null (Eve's checkout returns the descriptor's base ref,
 * which is null without a pull request), and the publisher then falls back to
 * `defaultBranch`, which is the ref the job's `baseSha` was already pinned to.
 */
export const assertExpectedBaseRef = (expectedBaseRef: string | undefined, baseRef: string | null): void => {
  if (expectedBaseRef !== undefined && baseRef !== null && baseRef !== expectedBaseRef) {
    throw new Error(`GitHub base ref mismatch: the anchor's base ref ${baseRef} differs from the job's base ref ${expectedBaseRef}`);
  }
};

const isCelld = (auth: SessionPrincipal | null | undefined): boolean =>
  auth?.authenticator === CELLD_AUTHENTICATOR;

/**
 * Resolves the attributes the publisher may trust for this turn.
 *
 * Eve's GitHub continuation token carries no execution nonce, so a later human
 * `@bot` comment on the same issue or PR delivers into the celld-initiated
 * session with only `auth.current` replaced. Reading the initiator's attributes
 * unconditionally would let a human turn inherit celld's `executionId`,
 * `expectedBaseSha`, `allowedPaths`, and `requiredTests`. Trusted attributes are
 * therefore read only when the current turn is itself the celld worker.
 */
export const trustedAttributes = (auth: {
  readonly current?: SessionPrincipal | null;
  readonly initiator?: SessionPrincipal | null;
}): { readonly kind: "celld"; readonly attributes: SessionPrincipal["attributes"] }
  | { readonly kind: "human" }
  | { readonly kind: "suppressed" } => {
  if (isCelld(auth.current)) return { kind: "celld", attributes: auth.current?.attributes ?? {} };
  // A celld-initiated session must not publish on a turn it did not initiate.
  if (isCelld(auth.initiator)) return { kind: "suppressed" };
  return { kind: "human" };
};

/** Structural subset of the channel and session context this handler reads. */
export interface PublishTurnInput {
  readonly channel: {
    readonly github: DraftPrPublishInput["github"];
    readonly state: DraftPrPublishState;
    readonly thread: { post(message: string): Promise<unknown> };
  };
  readonly session: {
    readonly id: string;
    readonly auth: { readonly current?: SessionPrincipal | null; readonly initiator?: SessionPrincipal | null };
  };
  getSandbox(): Promise<DraftPrPublishInput["sandbox"]>;
  /** Defaults to the on-disk receipts directory; tests inject an in-memory sink. */
  readonly receipts?: ReceiptSink;
}

const parseRequiredTests = (value: string | readonly string[] | undefined): readonly { readonly name: string; readonly command: string }[] | undefined => {
  if (value === undefined) return undefined;
  if (typeof value !== "string") throw new Error("Malformed required tests");
  let parsed: unknown;
  try { parsed = JSON.parse(value); } catch { throw new Error("Malformed required tests"); }
  if (!Array.isArray(parsed) || !parsed.every((test) => typeof test === "object" && test !== null &&
    "name" in test && typeof test.name === "string" && "command" in test && typeof test.command === "string")) {
    throw new Error("Malformed required tests");
  }
  return parsed as readonly { readonly name: string; readonly command: string }[];
};

const quote = (value: string): string => `'${value.replaceAll("'", "'\\''")}'`;

/**
 * Runs the required tests and publishes a draft PR for one completed turn.
 *
 * Returns the publication so the caller can stamp a receipt with it. A `null`
 * return means there was nothing to publish, which is only tolerable outside
 * the celld path.
 */
const publishTrustedTurn = async (
  input: PublishTurnInput,
  attributes: SessionPrincipal["attributes"] | undefined,
): Promise<DraftPrPublishResult | null> => {
  const { channel, session } = input;
  const expectedBaseSha = typeof attributes?.expectedBaseSha === "string" ? attributes.expectedBaseSha : undefined;
  assertExpectedBaseSha(expectedBaseSha, channel.state.headSha);
  const expectedBaseRef = typeof attributes?.baseRef === "string" ? attributes.baseRef : undefined;
  assertExpectedBaseRef(expectedBaseRef, channel.state.baseRef);
  const checkoutPath = channel.state.checkoutPath;
  if (checkoutPath === null || channel.state.headSha === null ||
      (channel.state.baseRef === null && channel.state.defaultBranch === null)) return null;
  const sandbox = await input.getSandbox();
  const requiredTests = parseRequiredTests(attributes?.requiredTests);
  const tests: { name: string; command: string; exitCode: number }[] = [];
  if (requiredTests !== undefined) {
    let failedTest: string | undefined;
    for (const test of requiredTests) {
      const result = await sandbox.run({ command: `cd ${quote(checkoutPath)} && /bin/sh -lc ${quote(test.command)}` });
      tests.push({ ...test, exitCode: result.exitCode });
      if (result.exitCode !== 0 && failedTest === undefined) failedTest = `${test.name} (exit ${result.exitCode})`;
    }
    if (failedTest !== undefined) throw new Error(`Required test failed: ${failedTest}`);
  }
  const allowedPaths = attributes?.allowedPaths;
  const publicationId = typeof attributes?.executionId === "string" ? attributes.executionId : session.id;
  const publication = await publishDraftPr({
    github: channel.github,
    sandbox,
    state: channel.state,
    publicationId,
    ...(Array.isArray(allowedPaths) ? { allowedPaths } : {}),
    ...(requiredTests === undefined ? {} : { tests }),
  });
  if (publication === null && typeof attributes?.executionId === "string") throw new Error("Celld publication produced no changes");
  if (publication !== null) {
    const url = publication.pullRequest.htmlUrl ?? `https://github.com/${channel.github.repository.owner}/${channel.github.repository.name}/pull/${publication.pullRequest.number}`;
    await channel.thread.post(`Draft PR: ${url}`);
  }
  return publication;
};

const errorMessage = (cause: unknown): string =>
  cause instanceof Error ? cause.message : String(cause);

/**
 * Runs one completed turn's publication and, for a celld-trusted turn, records
 * a durable receipt of the outcome.
 *
 * Eve swallows a throwing adapter event handler and writes `turn.completed`
 * regardless, so the throw below is not observable from the worker's stream.
 * The receipt is: it is written before the throw is re-raised, and the worker
 * reads it rather than inferring publication from the event.
 */
export async function publishTurn(input: PublishTurnInput): Promise<void> {
  const { channel, session } = input;
  const trusted = trustedAttributes(session.auth);
  if (trusted.kind === "suppressed") return;
  if (trusted.kind === "human") {
    await publishTrustedTurn(input, undefined);
    return;
  }
  const attributes = trusted.attributes;
  const executionId = typeof attributes.executionId === "string" ? attributes.executionId : undefined;
  // Without an execution id there is nothing a worker could poll for, so this
  // stays on the plain path rather than writing an unaddressable receipt.
  if (executionId === undefined) {
    await publishTrustedTurn(input, attributes);
    return;
  }
  const receipts = input.receipts ?? fileReceiptSink();
  const stamp = (
    fields: Omit<PublicationReceipt, "executionId" | "headSha" | "sessionId" | "updatedAt">,
  ): PublicationReceipt => ({
    executionId,
    headSha: channel.state.headSha ?? "",
    sessionId: session.id,
    updatedAt: new Date().toISOString(),
    ...fields,
  });
  let publication: DraftPrPublishResult | null;
  try {
    publication = await publishTrustedTurn(input, attributes);
  } catch (cause) {
    await receipts.write(stamp({ status: "failed", error: errorMessage(cause) }));
    throw cause;
  }
  if (publication === null) {
    // `publishTrustedTurn` throws for a celld no-changes publication, so a null
    // here means the checkout state was never publishable at all.
    await receipts.write(stamp({ status: "failed", error: "Celld publication had no publishable checkout state" }));
    throw new Error("Celld publication had no publishable checkout state");
  }
  await receipts.write(stamp({
    status: "published",
    branch: publication.branch,
    commitSha: publication.commitSha,
    changedPaths: publication.changedPaths,
    pullRequestNumber: publication.pullRequest.number,
    ...(publication.pullRequest.htmlUrl === undefined ? {} : { pullRequestUrl: publication.pullRequest.htmlUrl }),
  }));
}

/**
 * Runs the delegate in a `finally` so a configured `turn.completed` handler
 * still observes the turn when publishing throws.
 */
export async function publishThenDelegate(
  publish: () => Promise<void>,
  delegate: (() => void | Promise<void>) | undefined,
): Promise<void> {
  try {
    await publish();
  } finally {
    await delegate?.();
  }
}

export function createGitHubChannel(config: GitHubChannelConfig = {}): ReturnType<typeof eveGitHubChannel> {
  const botName = config.botName ?? process.env.GITHUB_APP_SLUG;
  return eveGitHubChannel({
    ...config,
    credentials: config.credentials ?? credentials,
    ...(botName === undefined ? {} : { botName }),
    events: {
      ...config.events,
      "turn.completed": async (event, channel, ctx) => {
        await publishThenDelegate(
          () => publishTurn({ channel, session: ctx.session, getSandbox: () => ctx.getSandbox() }),
          config.events?.["turn.completed"] === undefined
            ? undefined
            : () => config.events?.["turn.completed"]?.(event, channel, ctx),
        );
      },
    },
  });
}

export { defaultGitHubAuth };

const githubChannel = createGitHubChannel();

export default githubChannel;
