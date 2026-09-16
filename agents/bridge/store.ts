import { Crypto, Effect, FileSystem, Path, Schema } from "effect";
import { BridgeError } from "./errors.ts";
import { BridgeRecord } from "./types.ts";

const JsonBridgeRecord = Schema.fromJsonString(BridgeRecord);
const encodeRecord = Schema.encodeEffect(JsonBridgeRecord);
const decodeRecord = Schema.decodeUnknownEffect(JsonBridgeRecord);
const encodeJson = Schema.encodeEffect(Schema.fromJsonString(Schema.Unknown));

const failure = (operation: string, cause: unknown) =>
  new BridgeError({
    operation,
    detail: cause instanceof Error ? cause.message : String(cause),
    retryable: false,
  });

const mapFileError = (operation: string) => (cause: unknown) => failure(operation, cause);

const ensureDirectory = (fs: FileSystem.FileSystem, path: string, operation: string) =>
  fs.makeDirectory(path, { recursive: true, mode: 0o700 }).pipe(Effect.mapError(mapFileError(operation)));

/** The caller owns the bridge's process lock across reads and writes. */
export const loadBridgeRecords = Effect.fn("loadBridgeRecords")(function* (root: string) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  yield* ensureDirectory(fs, root, "load bridge records");
  const records: BridgeRecord[] = [];
  for (const entry of (yield* fs.readDirectory(root).pipe(Effect.mapError(mapFileError("read bridge records directory")))).sort()) {
    if (/^[0-9a-f]{64}\.json\.[0-9a-f-]+\.tmp$/.test(entry)) {
      yield* fs.remove(path.join(root, entry)).pipe(Effect.mapError(mapFileError("remove bridge staging file")));
      continue;
    }
    if (!entry.endsWith(".json")) continue;
    const record = yield* decodeRecord(yield* fs.readFileString(path.join(root, entry)).pipe(Effect.mapError(mapFileError("read bridge record")))).pipe(
      Effect.mapError(mapFileError("decode bridge record")),
    );
    if (entry !== `${record.candidateId}.json`) {
      return yield* new BridgeError({ operation: "load bridge records", detail: `record identity does not match filename: ${entry}`, retryable: false });
    }
    records.push(record);
  }
  return records;
});

export const saveBridgeRecord = Effect.fn("saveBridgeRecord")(function* (root: string, record: BridgeRecord) {
  const path = yield* Path.Path;
  const valid = yield* Schema.decodeEffect(BridgeRecord)(record).pipe(
    Effect.mapError((cause) => failure("validate bridge record", cause)),
  );
  const encoded = yield* encodeRecord(valid).pipe(Effect.mapError(mapFileError("encode bridge record")));
  yield* writeAtomicFile(path.join(root, `${valid.candidateId}.json`), `${encoded}\n`);
});

/** Publish only complete bytes, durable before a coordinator submission can occur. */
export const writeAtomicFile = Effect.fn("writeAtomicFile")(function* (target: string, bytes: string | Uint8Array) {
  const fs = yield* FileSystem.FileSystem;
  const path = yield* Path.Path;
  const crypto = yield* Crypto.Crypto;
  const parent = path.dirname(target);
  yield* ensureDirectory(fs, parent, "prepare atomic write");
  const temporaryId = yield* crypto.randomUUIDv4.pipe(Effect.mapError(mapFileError("create bridge staging file")));
  const staging = `${target}.${temporaryId}.tmp`;
  const data = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  yield* Effect.gen(function*() {
    yield* Effect.scoped(Effect.gen(function*() {
      const handle = yield* fs.open(staging, { flag: "wx", mode: 0o600 }).pipe(Effect.mapError(mapFileError("open bridge staging file")));
      yield* handle.writeAll(data).pipe(Effect.mapError(mapFileError("write bridge staging file")));
      yield* handle.sync.pipe(Effect.mapError(mapFileError("sync bridge staging file")));
    }));
    yield* fs.rename(staging, target).pipe(Effect.mapError(mapFileError("publish bridge file")));
  }).pipe(Effect.onExit(() => fs.remove(staging, { force: true }).pipe(Effect.mapError(mapFileError("remove bridge staging file")))));
  yield* Effect.scoped(Effect.gen(function*() {
    const directory = yield* fs.open(parent, { flag: "r" }).pipe(Effect.mapError(mapFileError("open bridge directory")));
    yield* directory.sync.pipe(Effect.mapError(mapFileError("sync bridge directory")));
  }));
});

export const encodeBridgeJson = (value: unknown) =>
  encodeJson(value).pipe(Effect.mapError((cause) => failure("encode bridge JSON", cause)));
