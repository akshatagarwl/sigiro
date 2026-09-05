import { createHash } from "node:crypto";
import { defineChannel, DELETE, GET, POST } from "eve/channels";
import type { RouteHandlerArgs } from "eve/channels";
import { parseWorldJobBody, parseWorldResultBody, dispatchWorldJob, MAX_BODY_BYTES } from "../lib/celld-dispatch.ts";
import type { WorldChannelState } from "../lib/celld-dispatch.ts";
import { readReceipt, ReceiptStoreError, writeReceipt } from "../lib/publication-receipts.ts";
import { githubClient, EvidenceDigestMismatchError, prepareWorkspace, verifiedEvidence } from "../lib/workspace.ts";
import { publishJob } from "../lib/publication.ts";

const jsonResponse = (body: unknown, status = 200): Response => new Response(JSON.stringify(body), {
  status, headers: { "content-type": "application/json", "cache-control": "no-store" },
});

async function boundedBody(request: Request): Promise<Uint8Array | Response> {
  const contentLength = request.headers.get("content-length");
  if (contentLength !== null && (!/^\d+$/.test(contentLength) || Number(contentLength) > MAX_BODY_BYTES)) return jsonResponse({ error: "invalid request" }, 400);
  try {
    if (request.body === null) return new Uint8Array();
    const reader = request.body.getReader(); const chunks: Uint8Array[] = []; let size = 0;
    while (true) {
      const next = await reader.read(); if (next.done) break;
      size += next.value.byteLength;
      if (size > MAX_BODY_BYTES) { await reader.cancel("body limit"); return jsonResponse({ error: "invalid request" }, 400); }
      chunks.push(next.value);
    }
    const bytes = new Uint8Array(size); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return bytes;
  } catch { return jsonResponse({ error: "invalid request" }, 400); }
}

const executionHash = (id: string): void => { /* span attributes are optional in local tests */ void createHash("sha256").update(id).digest("hex"); };

async function handleJob(request: Request, { from, resolveSession }: RouteHandlerArgs<WorldChannelState>): Promise<Response> {
  const body = await boundedBody(request); if (body instanceof Response) return body;
  const job = parseWorldJobBody(body); if (job === null) return jsonResponse({ error: "invalid request" }, 400);
  executionHash(job.executionId);
  try {
    await verifiedEvidence(job);
    const receipt = await readReceipt(job.executionId);
    if (receipt !== undefined) return jsonResponse({ sessionId: receipt.sessionId }, 202);
    const existing = await resolveSession(job.executionId);
    if (existing !== undefined) return jsonResponse({ sessionId: existing.id }, 202);
    return jsonResponse(await dispatchWorldJob(job, from), 202);
  } catch (cause) {
    console.error("World job ingress failed", cause);
    return jsonResponse({ error: "handoff failed" }, 502);
  }
}

export async function handleResult(request: Request): Promise<Response> {
  const body = await boundedBody(request); if (body instanceof Response) return body;
  const query = parseWorldResultBody(body); if (query === null) return jsonResponse({ error: "invalid request" }, 400);
  const receipt = await readReceipt(query.executionId);
  return receipt === undefined ? jsonResponse({ status: "pending" }, 404) : jsonResponse(receipt);
}

export async function handleStatus(_request: Request, { resolveSession, params }: RouteHandlerArgs<WorldChannelState>): Promise<Response> {
  const executionId = params.executionId;
  if (executionId === undefined) return jsonResponse({ error: "invalid request" }, 400);
  const receipt = await readReceipt(executionId);
  if (receipt !== undefined) return jsonResponse({ executionId, status: receipt.status, sessionId: receipt.sessionId, receipt });
  const session = await resolveSession(executionId);
  if (session === undefined) return jsonResponse({ error: "job not found" }, 404);
  return jsonResponse({ executionId, status: "accepted", sessionId: session.id });
}

export async function handleCancel(_request: Request, { resolveSession, attachSession, params }: RouteHandlerArgs<WorldChannelState>): Promise<Response> {
  const executionId = params.executionId;
  if (executionId === undefined) return jsonResponse({ error: "invalid request" }, 400);
  const session = await resolveSession(executionId);
  if (session === undefined) return jsonResponse({ error: "job not found" }, 404);
  const result = await attachSession(session.id).cancel();
  return jsonResponse({ executionId, sessionId: session.id, ...result }, 202);
}

const initialState: WorldChannelState = { job: null, prepared: false, error: null };
async function failed(state: WorldChannelState, sessionId: string, cause: unknown): Promise<void> {
  const error = cause instanceof Error ? cause : new Error(String(cause)); state.error = error.message;
  if (state.job === null) throw error;
  try { await writeReceipt({ executionId: state.job.executionId, headSha: state.job.baseSha, sessionId, updatedAt: new Date().toISOString(), status: "failed", error: state.error }); }
  catch (receiptError) { throw new AggregateError([error, receiptError], "execution failed and failure receipt could not be written"); }
}

export default defineChannel({
  state: initialState,
  turnPolicy: "queue",
  context: (state: WorldChannelState) => ({ state }),
  metadata: (state: WorldChannelState) => ({ prepared: state.prepared, error: state.error }),
  routes: [
    POST("/internal/world/job", handleJob),
    POST("/internal/world/job/result", handleResult),
    GET("/internal/world/job/:executionId", handleStatus),
    DELETE("/internal/world/job/:executionId", handleCancel),
  ],
  events: {
    "turn.started": async (_event, { state }, ctx) => {
      try {
        if (state.job === null) throw new Error("World session has no job");
        if (state.error !== null) throw new Error(state.error);
        if (!state.prepared) { await prepareWorkspace(state.job, await ctx.getSandbox()); state.prepared = true; }
      } catch (cause) { await failed(state, ctx.session.id, cause); throw cause; }
    },
    "turn.completed": async (_event, { state }, ctx) => {
      try {
        if (!state.prepared || state.job === null || state.error !== null) throw new Error(state.error ?? "World checkout was not prepared");
        await publishJob({ job: state.job, session: ctx.session, github: githubClient(state.job.repository), sandbox: await ctx.getSandbox() });
      } catch (cause) { await failed(state, ctx.session.id, cause); throw cause; }
    },
    "turn.failed": async (event, { state }, ctx) => { await failed(state, ctx.session.id, new Error(event.message)); },
    "turn.cancelled": async (_event, { state }, ctx) => { await failed(state, ctx.session.id, new Error("Eve turn cancelled")); },
    "session.failed": async (event, { state }) => { await failed(state, event.sessionId, new Error(event.message)); },
  },
});
