import { defineChannel, POST } from "eve/channels";
import { authenticateWorker, dispatchCelldJob, MAX_BODY_BYTES, parseCelldJobBody, parseCelldResultBody } from "../lib/celld-dispatch.ts";
import { readReceipt } from "../lib/publication-receipts.ts";

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
    return new Uint8Array(await request.arrayBuffer());
  } catch {
    return jsonResponse({ error: "invalid request" }, 400);
  }
}

async function handleJob(request: Request, { to }: Parameters<NonNullable<ReturnType<typeof POST>['handler']>>[1]): Promise<Response> {
  const body = await boundedBody(request);
  if (body instanceof Response) return body;
  const job = parseCelldJobBody(body);
  if (job === null) return jsonResponse({ error: "invalid request" }, 400);

  try {
    return jsonResponse(await dispatchCelldJob(job, to), 202);
  } catch {
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

  const receipt = await readReceipt(query.executionId);
  if (receipt === undefined) return jsonResponse({ status: "pending" }, 404);
  return jsonResponse(receipt, 200);
}

export default defineChannel({
  routes: [
    POST("/internal/celld/job", handleJob),
    POST("/internal/celld/job/result", handleResult),
  ],
});
