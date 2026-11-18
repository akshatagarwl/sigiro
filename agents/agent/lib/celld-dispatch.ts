import type { ChannelSendOptions } from "eve/channels";
import { z } from "zod";

export interface WorldChannelState { job: WorldJob | null; prepared: boolean; error: string | null; }
type WorldFrom = (address: string) => {
  send(message: string, options?: ChannelSendOptions<WorldChannelState>): Promise<{ readonly id: string }>;
};

const MAX_BODY_BYTES = 256 * 1024;
const MAX_STRING = 8_192;
const MAX_SHORT_STRING = 512;
const boundedString = (max = MAX_STRING) => z.string().min(1).max(max);

export const worldJobSchema = z.object({
  executionId: boundedString(MAX_SHORT_STRING),
  incidentId: boundedString(MAX_SHORT_STRING),
  evidenceUri: boundedString(),
  evidenceSha256: z.string().regex(/^[0-9a-f]{64}$/),
  repository: z.string().regex(/^[^/\s]+\/[^/\s]+$/).max(MAX_SHORT_STRING),
  baseRef: boundedString(MAX_SHORT_STRING),
  baseSha: z.string().regex(/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/),
  prompt: boundedString(),
  tests: z.array(z.object({ name: boundedString(MAX_SHORT_STRING), command: boundedString() }).strict()).min(1).max(100),
  // Dependency installation for a cold sandbox. A Box VM ships language
  // runtimes but not this repository's packages, so without these the tests
  // cannot run. Supplied by the control plane alongside tests, never by the
  // model, and executed during workspace preparation before the first turn.
  setup: z.array(z.object({ name: boundedString(MAX_SHORT_STRING), command: boundedString() }).strict()).max(20).optional(),
  allowedPaths: z.array(boundedString(MAX_SHORT_STRING)).min(1).max(1_000),
}).strict();
export type WorldJob = z.infer<typeof worldJobSchema>;

export const worldResultQuerySchema = z.object({ executionId: boundedString(MAX_SHORT_STRING) }).strict();
export type WorldResultQuery = z.infer<typeof worldResultQuerySchema>;
export const parseWorldResultBody = (bytes: Uint8Array): WorldResultQuery | null => {
  if (bytes.byteLength > MAX_BODY_BYTES) return null;
  try { return worldResultQuerySchema.parse(JSON.parse(new TextDecoder().decode(bytes))); } catch { return null; }
};
export const parseWorldJobBody = (bytes: Uint8Array): WorldJob | null => {
  if (bytes.byteLength > MAX_BODY_BYTES) return null;
  try { return worldJobSchema.parse(JSON.parse(new TextDecoder().decode(bytes))); } catch { return null; }
};

export function messageForJob(job: WorldJob): string {
  return [
    `Execute job ${job.executionId} for repository ${job.repository}.`,
    `Before any edits, verify that git rev-parse HEAD equals exactly ${job.baseSha}. If it does not, stop.`,
    "Use only /workspace/repository in the Eve sandbox, through the provided sandbox tools.",
    `Read /workspace/sigiro-evidence.json before editing. It is the verified Sigiro evidence snapshot for incident ${job.incidentId}, SHA-256 ${job.evidenceSha256}.`,
    `Follow these allowed paths exclusively: ${job.allowedPaths.join(", ")}.`,
    `Base ref: ${job.baseRef}.`,
    `Prompt: ${job.prompt}`,
    ...(job.setup?.length
      ? ["Dependencies are already installed. Do not reinstall them unless a test failure proves they are missing."]
      : ["No dependencies were installed for you. If a test needs packages, install them yourself before running it."]),
    "Run every listed test:",
    job.tests.map(({ name, command }) => `- ${name}: ${command}`).join("\n"),
    "Leave all changes uncommitted so the Eve publisher can publish them.",
  ].join("\n");
}

export async function dispatchWorldJob(job: WorldJob, from: WorldFrom): Promise<{ readonly sessionId: string }> {
  const session = await from(job.executionId).send(messageForJob(job), { auth: { authenticator: "world-job", principalId: job.executionId, principalType: "job", attributes: { executionId: job.executionId } }, turnPolicy: "queue", state: { job, prepared: false, error: null } });
  return { sessionId: session.id };
}
export { MAX_BODY_BYTES };
