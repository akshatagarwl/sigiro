import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Predicate, Schema } from "effect";
import { PublicationReceipt } from "../../src/publication.ts";

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
export { PublicationReceipt } from "../../src/publication.ts";

export class ReceiptStoreError extends Schema.TaggedError<ReceiptStoreError>()("ReceiptStoreError", {
  operation: Schema.Literals(["read", "decode", "write"]),
  executionId: Schema.String,
  cause: Schema.Defect(),
}) {
  override get message(): string { return `Failed to ${this.operation} publication receipt for ${this.executionId}`; }
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
const receiptName = (executionId: string, status: PublicationReceipt["status"]): string =>
  `${createHash("sha256").update(executionId).digest("hex")}${status === "failed" ? ".failed" : ""}.json`;

const decodeReceipt = Schema.decodeUnknownSync(Schema.fromJsonString(PublicationReceipt));
export const parseReceipt = Schema.decodeUnknownSync(PublicationReceipt);

/** Only a missing file means pending; corrupt or inaccessible receipts fail. */
export const readReceipt = async (executionId: string, directory = receiptsDirectory()): Promise<PublicationReceipt | undefined> => {
  for (const status of ["published", "failed"] as const) {
    let raw: string;
    try {
      raw = await readFile(join(directory, receiptName(executionId, status)), "utf8");
    } catch (cause) {
      if (Predicate.hasProperty(cause, "code") && cause.code === "ENOENT") continue;
      throw new ReceiptStoreError({ operation: "read", executionId, cause });
    }
    try {
      const receipt = decodeReceipt(raw);
      if (receipt.executionId !== executionId || receipt.status !== status) throw new Error("Receipt identity or status does not match its storage slot");
      return receipt;
    } catch (cause) {
      throw new ReceiptStoreError({ operation: "decode", executionId, cause });
    }
  }
  return undefined;
};

/**
 * Success and failure have separate atomic files. Readers prefer publication,
 * so a concurrent failure writer cannot retract a PR that was already opened.
 * Replays may refresh publication evidence for the same deterministic PR.
 */
export const writeReceipt = async (receipt: PublicationReceipt, directory = receiptsDirectory()): Promise<void> => {
  const target = join(directory, receiptName(receipt.executionId, receipt.status));
  const temporary = `${target}.${process.pid}.${randomUUID()}.tmp`;
  try {
    const checked = parseReceipt(receipt);
    await mkdir(directory, { recursive: true, mode: 0o700 });
    await writeFile(temporary, `${JSON.stringify(checked)}\n`, { mode: 0o600 });
    await rename(temporary, target);
  } catch (cause) {
    try { await unlink(temporary); }
    catch (cleanupCause) {
      if (!Predicate.hasProperty(cleanupCause, "code") || cleanupCause.code !== "ENOENT") {
        throw new ReceiptStoreError({ operation: "write", executionId: receipt.executionId,
          cause: new AggregateError([cause, cleanupCause], "Receipt write and temporary-file cleanup failed", { cause }) });
      }
    }
    throw new ReceiptStoreError({ operation: "write", executionId: receipt.executionId, cause });
  }
};
