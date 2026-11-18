import { randomUUID } from "node:crypto";
import { mkdir, open, readFile, readdir, rename, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { Schema } from "effect";
import { BridgeRecord } from "./types.ts";

const validateRecord = Schema.decodeUnknownSync(BridgeRecord);
const decodeRecord = Schema.decodeUnknownSync(Schema.fromJsonString(BridgeRecord));

/** The caller owns the bridge's process lock across reads and writes. */
export async function loadBridgeRecords(root: string): Promise<ReadonlyArray<BridgeRecord>> {
  await mkdir(root, { recursive: true, mode: 0o700 });
  const records: BridgeRecord[] = [];
  for (const entry of (await readdir(root)).sort()) {
    // A killed process can leave unpublished staging files; they never represent jobs.
    if (/^[0-9a-f]{64}\.json\.[0-9a-f-]+\.tmp$/.test(entry)) {
      await rm(join(root, entry));
      continue;
    }
    if (!entry.endsWith(".json")) continue;
    const record = decodeRecord(await readFile(join(root, entry), "utf8"));
    if (entry !== `${record.candidateId}.json`) {
      throw new Error(`Bridge record identity does not match its filename: ${entry}`);
    }
    records.push(record);
  }
  return records;
}

export async function saveBridgeRecord(root: string, record: BridgeRecord): Promise<void> {
  const valid = validateRecord(record);
  await writeAtomicFile(join(root, `${valid.candidateId}.json`), `${JSON.stringify(valid)}\n`);
}

/** Publish only complete bytes, durable before a coordinator submission can occur. */
export async function writeAtomicFile(path: string, bytes: string | Uint8Array): Promise<void> {
  const parent = dirname(path);
  await mkdir(parent, { recursive: true, mode: 0o700 });
  const staging = `${path}.${randomUUID()}.tmp`;
  try {
    {
      await using handle = await open(staging, "wx", 0o600);
      await handle.writeFile(bytes);
      await handle.sync();
    }
    await rename(staging, path);
    await using directory = await open(parent, "r");
    await directory.sync();
  } catch (cause) {
    try {
      await rm(staging, { force: true });
    } catch (cleanup) {
      throw new AggregateError([cause, cleanup], `Atomic write and staging cleanup failed: ${path}`);
    }
    throw cause;
  }
}
