import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";

/**
 * Durable evidence that one execution's `turn.completed` handler reached its
 * end, or the reason it did not.
 *
 * Eve's `callAdapterEventHandler` (channel/adapter.js) wraps an authored event
 * handler in `try`/`catch` and only logs a throw; the event is still stamped and
 * written to the durable stream. An observed `turn.completed` therefore proves
 * only that the turn ended, never that the required tests ran or that a draft PR
 * was published. The worker reads this receipt instead of inferring publication
 * from the event.
 */
export interface PublicationReceipt {
  readonly executionId: string;
  readonly status: "published" | "failed";
  readonly error?: string;
  readonly branch?: string;
  readonly commitSha?: string;
  readonly changedPaths?: readonly string[];
  readonly pullRequestNumber?: number;
  readonly pullRequestUrl?: string;
  readonly headSha: string;
  readonly sessionId: string;
  readonly updatedAt: string;
}

/** The sink `publishTurn` writes through. Tests supply an in-memory one. */
export interface ReceiptSink {
  readonly write: (receipt: PublicationReceipt) => Promise<void>;
}

/** Default directory name, relative to the app working directory. */
export const DEFAULT_RECEIPTS_DIRNAME = ".celld-receipts";

export const receiptsDirectory = (): string =>
  process.env.CELLD_RECEIPTS_DIR ?? join(process.cwd(), DEFAULT_RECEIPTS_DIRNAME);

/**
 * An execution id is worker-supplied and bounded, but it is not a safe file
 * name: `job/1:2:3` would escape the directory. Hashing gives a fixed-width
 * name that cannot traverse, and the id is carried inside the record anyway.
 */
const receiptName = (executionId: string): string =>
  `${createHash("sha256").update(executionId).digest("hex")}.json`;

const isStringArray = (value: unknown): value is readonly string[] =>
  Array.isArray(value) && value.every((entry) => typeof entry === "string");

/**
 * Validates the shape read back off disk. A truncated or hand-edited file is
 * not a receipt, and returning `undefined` for it keeps the worker in its
 * "still pending" branch rather than letting a malformed record decide an
 * execution's fate.
 */
export const parseReceipt = (value: unknown): PublicationReceipt | undefined => {
  if (typeof value !== "object" || value === null) return undefined;
  const record = value as Record<string, unknown>;
  const { executionId, status, headSha, sessionId, updatedAt } = record;
  if (typeof executionId !== "string" || executionId.length === 0) return undefined;
  if (status !== "published" && status !== "failed") return undefined;
  if (typeof headSha !== "string" || typeof sessionId !== "string" || typeof updatedAt !== "string") return undefined;
  const optionalString = (key: "error" | "branch" | "commitSha" | "pullRequestUrl"): string | undefined => {
    const entry = record[key];
    return typeof entry === "string" ? entry : undefined;
  };
  const pullRequestNumber = typeof record.pullRequestNumber === "number" && Number.isInteger(record.pullRequestNumber)
    ? record.pullRequestNumber
    : undefined;
  const changedPaths = isStringArray(record.changedPaths) ? record.changedPaths : undefined;
  const error = optionalString("error");
  const branch = optionalString("branch");
  const commitSha = optionalString("commitSha");
  const pullRequestUrl = optionalString("pullRequestUrl");
  return {
    executionId,
    status,
    headSha,
    sessionId,
    updatedAt,
    ...(error === undefined ? {} : { error }),
    ...(branch === undefined ? {} : { branch }),
    ...(commitSha === undefined ? {} : { commitSha }),
    ...(changedPaths === undefined ? {} : { changedPaths }),
    ...(pullRequestNumber === undefined ? {} : { pullRequestNumber }),
    ...(pullRequestUrl === undefined ? {} : { pullRequestUrl }),
  };
};

export const readReceipt = async (executionId: string, directory = receiptsDirectory()): Promise<PublicationReceipt | undefined> => {
  let raw: string;
  try {
    raw = await readFile(join(directory, receiptName(executionId)), "utf8");
  } catch {
    return undefined;
  }
  try {
    return parseReceipt(JSON.parse(raw));
  } catch {
    return undefined;
  }
};

/**
 * A "published" receipt is immutable evidence: the branch, the commit, and the
 * pull request exist on GitHub whatever happens next in this process. A later
 * "failed" write is therefore ignored rather than allowed to retract a
 * publication that really happened. A replay that republishes may overwrite
 * "published" with "published", because that write describes the same
 * deterministic branch and pull request.
 */
export const writeReceipt = async (receipt: PublicationReceipt, directory = receiptsDirectory()): Promise<void> => {
  if (receipt.status === "failed" && (await readReceipt(receipt.executionId, directory))?.status === "published") return;
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const target = join(directory, receiptName(receipt.executionId));
  const temporary = `${target}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await writeFile(temporary, `${JSON.stringify(receipt)}\n`, { mode: 0o600 });
    await rename(temporary, target);
  } catch (cause) {
    await unlink(temporary).catch(() => {});
    throw cause;
  }
};

/** The production sink, backed by the receipts directory. */
export const fileReceiptSink = (directory = receiptsDirectory()): ReceiptSink => ({
  write: (receipt) => writeReceipt(receipt, directory),
});
