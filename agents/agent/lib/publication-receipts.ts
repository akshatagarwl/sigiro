import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { PublicationReceipt } from "../../src/publication.ts";

export { PublicationReceipt } from "../../src/publication.ts";
export class ReceiptStoreError extends Error {
  constructor(readonly operation: "read" | "decode" | "write", readonly executionId: string, cause: unknown) {
    super(`Failed to ${operation} publication receipt for ${executionId}`, { cause });
    this.name = "ReceiptStoreError";
  }
}

export const DEFAULT_RECEIPTS_DIRNAME = ".world-job-receipts";
export const receiptsDirectory = (): string => process.env.WORLD_JOB_RECEIPTS_DIR ?? join(process.cwd(), DEFAULT_RECEIPTS_DIRNAME);
const receiptName = (executionId: string, status: PublicationReceipt["status"]): string =>
  `${createHash("sha256").update(executionId).digest("hex")}${status === "failed" ? ".failed" : ""}.json`;

export const parseReceipt = (value: unknown): PublicationReceipt | undefined => {
  const parsed = PublicationReceipt.safeParse(value);
  return parsed.success ? parsed.data : undefined;
};

export const readReceipt = async (executionId: string, directory = receiptsDirectory()): Promise<PublicationReceipt | undefined> => {
  for (const status of ["published", "failed"] as const) {
    let raw: string;
    try { raw = await readFile(join(directory, receiptName(executionId, status)), "utf8"); }
    catch (cause) {
      if (cause instanceof Error && "code" in cause && cause.code === "ENOENT") continue;
      throw new ReceiptStoreError("read", executionId, cause);
    }
    let receipt: PublicationReceipt | undefined;
    try { receipt = parseReceipt(JSON.parse(raw)); }
    catch (cause) { throw new ReceiptStoreError("decode", executionId, cause); }
    if (receipt === undefined || receipt.executionId !== executionId || receipt.status !== status) {
      throw new ReceiptStoreError("decode", executionId, new Error("Receipt identity or status does not match its storage slot"));
    }
    return receipt;
  }
  return undefined;
};

export const writeReceipt = async (receipt: PublicationReceipt, directory = receiptsDirectory()): Promise<void> => {
  if (parseReceipt(receipt) === undefined) throw new ReceiptStoreError("write", receipt.executionId, new Error("invalid receipt"));
  const target = join(directory, receiptName(receipt.executionId, receipt.status));
  const temporary = `${target}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    await writeFile(temporary, `${JSON.stringify(receipt)}\n`, { mode: 0o600 });
    await rename(temporary, target);
  } catch (cause) {
    try { await unlink(temporary); } catch (cleanup) { if (!(cleanup instanceof Error && "code" in cleanup && cleanup.code === "ENOENT")) throw new ReceiptStoreError("write", receipt.executionId, new AggregateError([cause, cleanup])); }
    throw new ReceiptStoreError("write", receipt.executionId, cause);
  }
};
