import { createHash } from "node:crypto";
import { BunServices } from "@effect/platform-bun";
import { Context, DateTime, Effect, FileSystem, Layer, Path, Predicate, Schema } from "effect";
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

const stableHash = (value: unknown): string =>
  createHash("sha256").update(JSON.stringify(value)).digest("hex");

const recordName = (id: string): string => `${stableHash(id)}.json`;

/**
 * A truncated or hand-edited file fails loudly here rather than surfacing as a
 * confusing shape later. The record schema lives beside the type it validates,
 * so `request` and `receipt` are checked too; this module used to declare them
 * `Schema.Unknown` and then cast the result, which validated the envelope and
 * nothing inside it.
 */
const decodeRecord = Schema.decodeUnknownEffect(Schema.fromJsonString(JobRecord));

export const makeFileJobStore = (root: string): Layer.Layer<JobStore> =>
  Layer.effect(
    JobStore,
    Effect.gen(function*() {
      const fs = yield* Effect.service(FileSystem.FileSystem);
      const path = yield* Effect.service(Path.Path);
      const pathFor = (id: string) => path.join(root, recordName(id));

      const parse = Effect.fn("JobStore.parse")(function*(raw: string, operation: string) {
        return yield* decodeRecord(raw).pipe(
          Effect.mapError((cause) => new ExecutorError(operation, String(cause), cause)),
        );
      });

      const get = Effect.fn("JobStore.get")(function*(id: string) {
        const raw = yield* fs.readFileString(pathFor(id)).pipe(
          Effect.catchTag("PlatformError", (cause) =>
            cause.reason._tag === "NotFound"
              ? Effect.succeed(undefined)
              : Effect.fail(new ExecutorError("read job", String(cause), cause))),
        );
        return Predicate.isUndefined(raw) ? undefined : yield* parse(raw, "read job");
      });

      /** Writes to a unique temporary file and renames, so a reader never sees a partial record. */
      const put = Effect.fn("JobStore.put")(function*(record: JobRecord) {
        yield* fs.makeDirectory(root, { recursive: true, mode: 0o700 }).pipe(
          Effect.catchTag("PlatformError", (cause) =>
            new ExecutorError("write job", String(cause), cause)),
        );
        const target = pathFor(record.executionId);
        const temporary = `${target}.${process.pid}.${crypto.randomUUID()}.tmp`;
        yield* fs.writeFileString(temporary, `${JSON.stringify(record)}\n`, { mode: 0o600 }).pipe(
          Effect.andThen(fs.rename(temporary, target)),
          Effect.catchTag("PlatformError", (cause) =>
            new ExecutorError("write job", String(cause), cause)),
          Effect.onExit(() => fs.remove(temporary, { force: true }).pipe(
            Effect.mapError((cause) => new ExecutorError("remove job staging file", String(cause), cause)),
          )),
        );
      });

      const create = Effect.fn("JobStore.create")(
        function*(request: JobRequest): Effect.fn.Return<CreateRecordResult, ExecutorError | RequestConflict> {
          yield* fs.makeDirectory(root, { recursive: true, mode: 0o700 }).pipe(
            Effect.catchTag("PlatformError", (cause) =>
              new ExecutorError("create job", String(cause), cause)),
          );
          const id = executionId(request);
          const target = pathFor(id);
          const requestHash = stableHash(request);
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
          const staging = `${target}.${process.pid}.${crypto.randomUUID()}.tmp`;
          return yield* fs.writeFileString(staging, `${JSON.stringify(record)}\n`, { mode: 0o600 }).pipe(
            Effect.mapError((cause) => new ExecutorError("create job", String(cause), cause)),
            Effect.andThen(fs.link(staging, target).pipe(
              Effect.as<CreateRecordResult>({ record, created: true }),
              Effect.catchTag("PlatformError", (cause) =>
                cause.reason._tag === "AlreadyExists"
                  ? Effect.gen(function*() {
                    const existing = yield* get(id);
                    if (Predicate.isUndefined(existing)) {
                      return yield* new ExecutorError("create job", "record vanished after link conflict");
                    }
                    if (existing.requestHash !== requestHash) {
                      return yield* new RequestConflict(id);
                    }
                    return { record: existing, created: false } satisfies CreateRecordResult;
                  })
                  : Effect.fail(new ExecutorError("create job", String(cause), cause))),
            )),
            Effect.onExit(() => fs.remove(staging, { force: true }).pipe(
              Effect.mapError((cause) => new ExecutorError("remove job staging file", String(cause), cause)),
            )),
          );
        },
      );

      const pending = Effect.fn("JobStore.pending")(function*() {
        const entries = yield* fs.readDirectory(root).pipe(
          Effect.catchTag("PlatformError", (cause) =>
            cause.reason._tag === "NotFound"
              ? Effect.succeed<ReadonlyArray<string>>([])
              : Effect.fail(new ExecutorError("list jobs", String(cause), cause))),
        );
        const records: Array<JobRecord> = [];
        for (const entry of entries) {
          if (!entry.endsWith(".json")) continue;
          const raw = yield* fs.readFileString(path.join(root, entry)).pipe(
            Effect.catchTag("PlatformError", (cause) =>
              Effect.fail(new ExecutorError("read job", String(cause), cause))),
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
