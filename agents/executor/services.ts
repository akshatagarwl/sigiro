import { BunServices } from "@effect/platform-bun";
import { Context, Crypto, DateTime, Effect, FileSystem, Layer, Path, Predicate, Schema } from "effect";
import { sha256 } from "../src/signing.ts";
import { ExecutorError, RequestConflict } from "./errors.ts";
import type { JobRequest } from "./types.ts";
import { executionId, JobRecord } from "./types.ts";

interface CreateRecordResult {
  readonly record: JobRecord;
  readonly created: boolean;
}

export class JobStore extends Context.Service<JobStore, {
  readonly create: (request: JobRequest) => Effect.Effect<CreateRecordResult, ExecutorError | RequestConflict>;
  readonly get: (id: string) => Effect.Effect<JobRecord | undefined, ExecutorError>;
  readonly pending: () => Effect.Effect<ReadonlyArray<JobRecord>, ExecutorError>;
  readonly put: (record: JobRecord) => Effect.Effect<void, ExecutorError>;
}>()(
  "sigiro/agents/JobStore",
) {}


/**
 * A truncated or hand-edited file fails loudly here rather than surfacing as a
 * confusing shape later. The record schema lives beside the type it validates,
 * so `request` and `receipt` are checked too; this module used to declare them
 * `Schema.Unknown` and then cast the result, which validated the envelope and
 * nothing inside it.
 */
const decodeRecord = Schema.decodeUnknownEffect(Schema.fromJsonString(JobRecord));
const encodeRecord = Schema.encodeEffect(Schema.fromJsonString(Schema.Unknown));

export const makeFileJobStore = (root: string): Layer.Layer<JobStore> =>
  Layer.effect(
    JobStore,
    Effect.gen(function*() {
      const fs = yield* Effect.service(FileSystem.FileSystem);
      const path = yield* Effect.service(Path.Path);
      const crypto = yield* Effect.service(Crypto.Crypto);
      const stableHash = Effect.fn("JobStore.stableHash")(function* (value: unknown) {
        const encoded = yield* Schema.encodeEffect(Schema.fromJsonString(Schema.Unknown))(value).pipe(
          Effect.mapError((cause) => ExecutorError.from("hash job", String(cause), cause)),
        );
        return yield* sha256(encoded);
      });
      const pathFor = Effect.fn("JobStore.pathFor")(function* (id: string) {
        return path.join(root, `${yield* stableHash(id)}.json`);
      });

      const parse = Effect.fn("JobStore.parse")(function*(raw: string, operation: string) {
        return yield* decodeRecord(raw).pipe(
          Effect.mapError((cause) => ExecutorError.from(operation, String(cause), cause)),
        );
      });

      const get = Effect.fn("JobStore.get")(function*(id: string) {
        const raw = yield* fs.readFileString(yield* pathFor(id)).pipe(
          Effect.catchTag("PlatformError", (cause) =>
            cause.reason._tag === "NotFound"
              ? Effect.succeed(undefined)
              : Effect.fail(ExecutorError.from("read job", String(cause), cause))),
        );
        return Predicate.isUndefined(raw) ? undefined : yield* parse(raw, "read job");
      });

      /** Writes to a unique temporary file and renames, so a reader never sees a partial record. */
      const put = Effect.fn("JobStore.put")(function*(record: JobRecord) {
        yield* fs.makeDirectory(root, { recursive: true, mode: 0o700 }).pipe(
          Effect.catchTag("PlatformError", (cause) =>
            ExecutorError.from("write job", String(cause), cause)),
        );
        const target = yield* pathFor(record.executionId);
        const temporaryId = yield* crypto.randomUUIDv4.pipe(Effect.mapError((cause) => ExecutorError.from("write job", String(cause), cause)));
        const temporary = `${target}.${process.pid}.${temporaryId}.tmp`;
        const encoded = yield* encodeRecord(record).pipe(Effect.mapError((cause) => ExecutorError.from("write job", String(cause), cause)));
        yield* fs.writeFileString(temporary, `${encoded}\n`, { mode: 0o600 }).pipe(
          Effect.andThen(fs.rename(temporary, target)),
          Effect.catchTag("PlatformError", (cause) =>
            ExecutorError.from("write job", String(cause), cause)),
          Effect.onExit(() => fs.remove(temporary, { force: true }).pipe(
            Effect.mapError((cause) => ExecutorError.from("remove job staging file", String(cause), cause)),
          )),
        );
      });

      const create = Effect.fn("JobStore.create")(
        function*(request: JobRequest): Effect.fn.Return<CreateRecordResult, ExecutorError | RequestConflict> {
          yield* fs.makeDirectory(root, { recursive: true, mode: 0o700 }).pipe(
            Effect.catchTag("PlatformError", (cause) =>
              ExecutorError.from("create job", String(cause), cause)),
          );
          const id = executionId(request);
          const target = yield* pathFor(id);
          const requestHash = yield* stableHash(request);
          const now = DateTime.formatIso(yield* DateTime.now);
          const record: JobRecord = {
            executionId: id,
            requestHash,
            request,
            status: "accepted",
            acceptedAt: now,
            updatedAt: now,
          };
          // Write the whole record first, then link it into place. `open(target,"wx")`
          // is atomic for creation but makes the file visible while it is still
          // empty, so a concurrent duplicate submission read `""` and died on
          // JSON.parse. `link` publishes only a fully written file.
          const stagingId = yield* crypto.randomUUIDv4.pipe(Effect.mapError((cause) => ExecutorError.from("create job", String(cause), cause)));
          const staging = `${target}.${process.pid}.${stagingId}.tmp`;
          const encoded = yield* encodeRecord(record).pipe(Effect.mapError((cause) => ExecutorError.from("write job", String(cause), cause)));
          return yield* fs.writeFileString(staging, `${encoded}\n`, { mode: 0o600 }).pipe(
            Effect.mapError((cause) => ExecutorError.from("create job", String(cause), cause)),
            Effect.andThen(fs.link(staging, target).pipe(
              Effect.as<CreateRecordResult>({ record, created: true }),
              Effect.catchTag("PlatformError", (cause) =>
                cause.reason._tag === "AlreadyExists"
                  ? Effect.gen(function*() {
                    const existing = yield* get(id);
                    if (Predicate.isUndefined(existing)) {
                      return yield* ExecutorError.from("create job", "record vanished after link conflict");
                    }
                    if (existing.requestHash !== requestHash) {
                      return yield* RequestConflict.from(id);
                    }
                    return { record: existing, created: false } satisfies CreateRecordResult;
                  })
                  : Effect.fail(ExecutorError.from("create job", String(cause), cause))),
            )),
            Effect.onExit(() => fs.remove(staging, { force: true }).pipe(
              Effect.mapError((cause) => ExecutorError.from("remove job staging file", String(cause), cause)),
            )),
          );
        },
      );

      const pending = Effect.fn("JobStore.pending")(function*() {
        const entries = yield* fs.readDirectory(root).pipe(
          Effect.catchTag("PlatformError", (cause) =>
            cause.reason._tag === "NotFound"
              ? Effect.succeed<ReadonlyArray<string>>([])
              : Effect.fail(ExecutorError.from("list jobs", String(cause), cause))),
        );
        const records: Array<JobRecord> = [];
        for (const entry of entries) {
          if (!entry.endsWith(".json")) continue;
          const raw = yield* fs.readFileString(path.join(root, entry)).pipe(
            Effect.catchTag("PlatformError", (cause) =>
              Effect.fail(ExecutorError.from("read job", String(cause), cause))),
          );
          const record = yield* parse(raw, "read job");
          if (record.reportedAt === undefined) {
            records.push(record);
          }
        }
        return records;
      });

      return JobStore.of({ create, get, pending, put });
    }),
  ).pipe(Layer.provide(BunServices.layer));
