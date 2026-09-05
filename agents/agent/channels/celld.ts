import { defineChannel, POST } from "eve/channels";
import type { RouteHandlerArgs } from "eve/channels";
import { authenticateWorker, dispatchCelldJob, MAX_BODY_BYTES, parseCelldJobBody, parseCelldResultBody } from "../lib/celld-dispatch.ts";
import type { CelldChannelState } from "../lib/celld-dispatch.ts";
import { readReceipt, ReceiptStoreError, writeReceipt } from "../lib/publication-receipts.ts";
import { githubClient, EvidenceDigestMismatchError, prepareWorkspace, verifiedEvidence } from "../lib/workspace.ts";
import { publishJob } from "../lib/publication.ts";

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/**
 * Reads a bounded body after the worker bearer check. Returns a `Response` for
 * every refusal so a caller cannot mistake a rejected request for an empty one.
 */
async function boundedBody(request: Request): Promise<Uint8Array | Response> {
  if (!authenticateWorker(request.headers.get("authorization"))) return jsonResponse({ error: "unauthorized" }, 401);

  const contentLength = request.headers.get("content-length");
  if (contentLength !== null && (!/^\d+$/.test(contentLength) || Number(contentLength) > MAX_BODY_BYTES)) {
    return jsonResponse({ error: "invalid request" }, 400);
  }

  try {
    if (request.body === null) return new Uint8Array();
    const reader = request.body.getReader();
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      while (true) {
        const next = await reader.read();
        if (next.done) break;
        size += next.value.byteLength;
        if (size > MAX_BODY_BYTES) {
          await reader.cancel("Request exceeded the body limit");
          return jsonResponse({ error: "invalid request" }, 400);
        }
        chunks.push(next.value);
      }
    } finally {
      reader.releaseLock();
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return bytes;
  } catch {
    return jsonResponse({ error: "invalid request" }, 400);
  }
}

async function handleJob(request: Request, { from, resolveSession }: RouteHandlerArgs<CelldChannelState>): Promise<Response> {
  const body = await boundedBody(request);
  if (body instanceof Response) return body;
  const job = parseCelldJobBody(body);
  if (job === null) return jsonResponse({ error: "invalid request" }, 400);
  const span = trace.getActiveSpan();
  span?.setAttribute("execution.hash", createHash("sha256").update(job.executionId).digest("hex"));

  try {
    await verifiedEvidence(job);
    const receipt = await readReceipt(job.executionId);
    if (receipt !== undefined) return jsonResponse({ sessionId: receipt.sessionId }, 202);
    const existing = await resolveSession(job.executionId);
    if (existing !== undefined) return jsonResponse({ sessionId: existing.id }, 202);
    return jsonResponse(await dispatchCelldJob(job, from), 202);
  } catch (cause) {
    span?.setAttribute("error.type", cause instanceof EvidenceDigestMismatchError ? "evidence_digest_mismatch" : cause instanceof ReceiptStoreError ? `receipt_${cause.operation}_failed` : "celld_handoff_failed");
    console.error("Celld handoff failed", cause);
    return jsonResponse({ error: "handoff failed" }, 502);
  }
}

/**
 * Reports whether one execution's `turn.completed` handler published.
 *
 * Eve swallows a throwing adapter event handler, so `turn.completed` on the
 * worker's stream proves only that the turn ended. This route serves the
 * durable receipt the handler wrote instead. It touches no GitHub API and holds
 * no credential beyond the worker bearer check above.
 */
export async function handleResult(request: Request): Promise<Response> {
  const body = await boundedBody(request);
  if (body instanceof Response) return body;
  const query = parseCelldResultBody(body);
  if (query === null) return jsonResponse({ error: "invalid request" }, 400);
  const span = trace.getActiveSpan();
  span?.setAttribute("execution.hash", createHash("sha256").update(query.executionId).digest("hex"));
  try {
    const receipt = await readReceipt(query.executionId);
    if (receipt === undefined) return jsonResponse({ status: "pending" }, 404);
    return jsonResponse(receipt, 200);
  } catch (cause) {
    span?.setAttribute("error.type", cause instanceof ReceiptStoreError ? `receipt_${cause.operation}_failed` : "receipt_result_failed");
    throw cause;
  }
}

const initialState: CelldChannelState = { job: null, prepared: false, error: null };

async function failed(state: CelldChannelState, sessionId: string, cause: unknown): Promise<void> {
  const error = cause instanceof Error ? cause : new Error(typeof cause === "string" ? cause : "Eve execution failed with a non-Error cause", { cause });
  state.error = error.message;
  if (state.job === null) throw new Error("Celld session has no job", { cause: error });
  try {
    await writeReceipt({ executionId: state.job.executionId, headSha: state.job.baseSha, sessionId, updatedAt: new Date().toISOString(), status: "failed", error: state.error });
  } catch (receiptError) {
    throw new AggregateError([error, receiptError], "Eve execution failed and its failure receipt could not be written", { cause: error });
  }
}

export default defineChannel({
  state: initialState,
  turnPolicy: "queue",
  context: (state: CelldChannelState) => ({ state }),
  metadata: (state: CelldChannelState) => ({ prepared: state.prepared, error: state.error }),
  routes: [
    POST("/internal/celld/job", handleJob),
    POST("/internal/celld/job/result", handleResult),
  ],
  events: {
    "turn.started": async (_event, { state }, ctx) => {
      try {
        if (state.job === null) throw new Error("Celld session has no job");
        if (state.error !== null) throw new Error(state.error);
        if (!state.prepared) {
          await prepareWorkspace(state.job, await ctx.getSandbox());
          state.prepared = true;
        }
      } catch (cause) {
        await failed(state, ctx.session.id, cause);
        throw cause;
      }
    },
    "turn.completed": async (_event, { state }, ctx) => {
      try {
        if (!state.prepared || state.job === null || state.error !== null) throw new Error(state.error ?? "Celld checkout was not prepared");
        await publishJob({
          job: state.job, session: ctx.session,
          github: githubClient(state.job.repository), sandbox: await ctx.getSandbox(),
        });
      } catch (cause) {
        await failed(state, ctx.session.id, cause);
        throw cause;
      }
    },
    "turn.failed": async (event, { state }, ctx) => { await failed(state, ctx.session.id, new Error(event.message, { cause: event })); },
    "turn.cancelled": async (_event, { state }, ctx) => { await failed(state, ctx.session.id, new Error("Eve turn cancelled")); },
    "session.failed": async (event, { state }) => { await failed(state, event.sessionId, new Error(event.message, { cause: event })); },
  },
});
import { createHash } from "node:crypto";
import { trace } from "@opentelemetry/api";
