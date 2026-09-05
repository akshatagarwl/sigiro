import { timingSafeEqual } from "node:crypto";
import { z } from "zod";
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

/** Structural DI boundary: the dispatch needs only Eve's address-bound send. */
type CelldFrom = (address: string) => {
  send(message: string, options: ChannelSendOptions<CelldChannelState>): Promise<{ readonly id: string }>;
};

const MAX_BODY_BYTES = 256 * 1024;
const MAX_STRING = 8_192;
const MAX_SHORT_STRING = 512;
const MAX_TESTS = 100;
const MAX_PATHS = 1_000;

const boundedString = (max = MAX_STRING) => z.string().min(1).max(max);

export const celldJobSchema = z.object({
  executionId: boundedString(MAX_SHORT_STRING),
  incidentId: boundedString(MAX_SHORT_STRING),
  evidenceUri: boundedString(MAX_STRING),
  evidenceSha256: z.string().regex(/^[0-9a-f]{64}$/),
  workerId: boundedString(MAX_SHORT_STRING),
  repository: z.string().regex(/^[^/\s]+\/[^/\s]+$/).max(MAX_SHORT_STRING),
  baseRef: boundedString(MAX_SHORT_STRING),
  baseSha: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/),
  prompt: boundedString(MAX_STRING),
  tests: z.array(z.object({ name: boundedString(MAX_SHORT_STRING), command: boundedString(MAX_STRING) }).strict()).min(1).max(MAX_TESTS),
  allowedPaths: z.array(boundedString(MAX_SHORT_STRING)).min(1).max(MAX_PATHS),
}).strict();

export type CelldJob = z.infer<typeof celldJobSchema>;

/** The result route's request body: one bounded execution id and nothing else. */
export const celldResultQuerySchema = z.object({
  executionId: boundedString(MAX_SHORT_STRING),
}).strict();

export type CelldResultQuery = z.infer<typeof celldResultQuerySchema>;

export function parseCelldResultBody(bytes: Uint8Array): CelldResultQuery | null {
  if (bytes.byteLength > MAX_BODY_BYTES) return null;
  try {
    return celldResultQuerySchema.parse(JSON.parse(new TextDecoder().decode(bytes)));
  } catch {
    return null;
  }
}

export function authenticateWorker(authorization: string | null | undefined, expectedToken?: string): boolean {
  const token = expectedToken ?? process.env.EVE_WORKER_TOKEN;
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
      // Publication must retain this pinned base ref.
      baseRef: job.baseRef,
      allowedPaths: job.allowedPaths,
      // Keep the policy data in a bounded scalar attribute so it survives Eve's
      // forwarded-principal serialization without trusting model prose.
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
  if (bytes.byteLength > MAX_BODY_BYTES) return null;
  try {
    return celldJobSchema.parse(JSON.parse(new TextDecoder().decode(bytes)));
  } catch {
    return null;
  }
}

export async function dispatchCelldJob(
  job: CelldJob,
  from: CelldFrom,
): Promise<{ readonly sessionId: string }> {
  const session = await from(job.executionId).send(messageForJob(job), {
    auth: authForJob(job),
    turnPolicy: "queue",
    state: { job, prepared: false, error: null },
  });
  return { sessionId: session.id };
}

export { MAX_BODY_BYTES };
