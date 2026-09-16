// oxlint-disable-next-line effecttsgo/node-builtin-import -- Effect Crypto has no timing-safe equality primitive.
import { timingSafeEqual } from "node:crypto";
import { Config, Effect, Option, Schema } from "effect";
import type { ChannelSendOptions } from "eve/channels";

export interface CelldAuthContext {
  readonly attributes: Readonly<Record<string, string | readonly string[]>>;
  readonly authenticator: string;
  readonly principalId: string;
  readonly principalType: string;
}

export interface CelldChannelState {
  job: CelldJob | null;
  prepared: boolean;
  error: string | null;
}

type CelldFrom = (address: string) => {
  send(message: string, options: ChannelSendOptions<CelldChannelState>): Promise<{ readonly id: string }>;
};

const MAX_BODY_BYTES = 256 * 1024;
const MAX_STRING = 8_192;
const MAX_SHORT_STRING = 512;
const MAX_TESTS = 100;
const MAX_PATHS = 1_000;

const boundedString = (max: number = MAX_STRING) => Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(max));

const sha256 = Schema.String.check(Schema.isPattern(/^[0-9a-f]{64}$/));
const baseSha = Schema.String.check(Schema.isPattern(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/));
const repository = Schema.String.check(Schema.isPattern(/^[^/\s]+\/[^/\s]+$/), Schema.isMaxLength(MAX_SHORT_STRING));

const CelldTest = Schema.Struct({
  name: boundedString(MAX_SHORT_STRING),
  command: boundedString(),
});

export const celldJobSchema = Schema.Struct({
  executionId: boundedString(MAX_SHORT_STRING),
  incidentId: boundedString(MAX_SHORT_STRING),
  evidenceUri: boundedString(),
  evidenceSha256: sha256,
  workerId: boundedString(MAX_SHORT_STRING),
  repository,
  baseRef: boundedString(MAX_SHORT_STRING),
  baseSha,
  prompt: boundedString(),
  tests: Schema.Array(CelldTest).check(Schema.isMinLength(1), Schema.isMaxLength(MAX_TESTS)),
  allowedPaths: Schema.Array(boundedString(MAX_SHORT_STRING)).check(Schema.isMinLength(1), Schema.isMaxLength(MAX_PATHS)),
});

export type CelldJob = typeof celldJobSchema.Type;

export const celldResultQuerySchema = Schema.Struct({
  executionId: boundedString(MAX_SHORT_STRING),
});

export type CelldResultQuery = typeof celldResultQuerySchema.Type;

const decodeJson = (bytes: Uint8Array): Option.Option<unknown> => {
  if (bytes.byteLength > MAX_BODY_BYTES) return Option.none();
  return Schema.decodeOption(Schema.fromJsonString(Schema.Unknown))(new TextDecoder().decode(bytes));
};

export function parseCelldResultBody(bytes: Uint8Array): CelldResultQuery | null {
  const json = decodeJson(bytes);
  if (Option.isNone(json)) return null;
  const decoded = Schema.decodeUnknownOption(celldResultQuerySchema, { onExcessProperty: "error" })(json.value);
  return Option.isSome(decoded) ? decoded.value : null;
}

export function authenticateWorker(authorization: string | null | undefined, expectedToken?: string): boolean {
  const configured = Effect.runSync(Config.String("EVE_WORKER_TOKEN").pipe(Config.option));
  const token = expectedToken ?? Option.getOrUndefined(configured);
  if (!token || !authorization?.startsWith("Bearer ")) return false;
  const supplied = authorization.slice("Bearer ".length);
  if (!supplied || supplied.length !== token.length) return false;
  return timingSafeEqual(Buffer.from(supplied), Buffer.from(token));
}

export function authForJob(job: CelldJob): CelldAuthContext {
  return {
    authenticator: "celld-worker-token",
    principalId: job.workerId,
    principalType: "worker",
    attributes: {
      executionId: job.executionId,
      incidentId: job.incidentId,
      evidenceUri: job.evidenceUri,
      evidenceSha256: job.evidenceSha256,
      expectedBaseSha: job.baseSha,
      baseRef: job.baseRef,
      allowedPaths: job.allowedPaths,
      requiredTests: JSON.stringify(job.tests),
    },
  };
}

export function messageForJob(job: CelldJob): string {
  const tests = job.tests.map(({ name, command }) => `- ${name}: ${command}`).join("\n");
  return [
    `Execute worker job ${job.executionId} for repository ${job.repository}.`,
    `Before any edits, verify that git rev-parse HEAD equals exactly ${job.baseSha}. If it does not, stop.`,
    `Use only /workspace/repository in the Eve sandbox, through the provided sandbox tools.`,
    `Read /workspace/sigiro-evidence.json before editing. It is the verified Sigiro evidence snapshot for incident ${job.incidentId}, SHA-256 ${job.evidenceSha256}.`,
    `Follow these allowed paths exclusively: ${job.allowedPaths.join(", ")}.`,
    `Base ref: ${job.baseRef}.`,
    `Prompt: ${job.prompt}`,
    `Run every listed test:`,
    tests,
    `Leave all changes uncommitted so the Eve publisher can publish them.`,
  ].join("\n");
}

export function parseCelldJobBody(bytes: Uint8Array): CelldJob | null {
  const json = decodeJson(bytes);
  if (Option.isNone(json)) return null;
  const decoded = Schema.decodeUnknownOption(celldJobSchema, { onExcessProperty: "error" })(json.value);
  return Option.isSome(decoded) ? decoded.value : null;
}

export class CelldDispatchError extends Schema.TaggedError<CelldDispatchError>()("CelldDispatchError", {
  cause: Schema.Unknown,
}) {}

const dispatchCelldJobEffect = Effect.fn("dispatchCelldJob")(function* (job: CelldJob, from: CelldFrom) {
  const session = yield* Effect.tryPromise({
    try: () => from(job.executionId).send(messageForJob(job), {
      auth: authForJob(job),
      turnPolicy: "queue",
      state: { job, prepared: false, error: null },
    }),
    catch: (cause) => new CelldDispatchError({ cause }),
  });
  return { sessionId: session.id };
});

export function dispatchCelldJob(job: CelldJob, from: CelldFrom): Promise<{ readonly sessionId: string }> {
  return Effect.runPromise(dispatchCelldJobEffect(job, from));
}

export { MAX_BODY_BYTES };
