// Runtime-gate Worker: runs an ordered list of steps (SQL, catalog publish, catalog restore)
// inside ONE request, because Workers isolates are not sticky between requests.
// Deployed only by test/runtime-gate-e2e.ts and deleted afterwards.
import { fileSystem } from "@ducklings/workers-ducklake";

import { lakeConnection } from "./runtime";

interface Env {
  LAKE: R2Bucket;
  GATE_TOKEN: string;
}

export type Step =
  | { sql: string }
  | { publish: { database: string; catalog: string; key: string } }
  | { restore: { key: string; catalog: string } };

export type Outcome =
  | { step: Step; ok: true; rows?: unknown[] }
  | { step: Step; ok: false; error: string };

const json = (value: unknown, status = 200) =>
  new Response(
    JSON.stringify(value, (_k, v) =>
      typeof v === "bigint" ? v.toString() : v
    ),
    { status, headers: { "content-type": "application/json" } }
  );

async function runStep(env: Env, step: Step): Promise<unknown[] | undefined> {
  const c = await lakeConnection(env.LAKE);
  if ("sql" in step) return c.query(step.sql);
  if ("publish" in step) {
    await c.execute(`CHECKPOINT ${step.publish.database}`);
    await env.LAKE.put(
      step.publish.key,
      fileSystem().readFile(step.publish.catalog)
    );
    return undefined;
  }
  if ("restore" in step) {
    const object = await env.LAKE.get(step.restore.key);
    if (!object) throw new Error(`no catalog at ${step.restore.key}`);
    fileSystem().writeFile(
      step.restore.catalog,
      new Uint8Array(await object.arrayBuffer())
    );
    return undefined;
  }
  throw new Error(`unknown step ${JSON.stringify(step)}`);
}

const STEP_KINDS = ["sql", "publish", "restore"];

/** Parse the request body at the boundary: an array of objects with exactly one known step kind. */
function parseSteps(body: unknown): Step[] {
  if (!Array.isArray(body)) throw new Error("steps must be a JSON array");
  for (const step of body) {
    const kinds = typeof step === "object" && step !== null ? Object.keys(step) : [];
    if (kinds.length !== 1 || !STEP_KINDS.includes(kinds[0]))
      throw new Error(`each step needs exactly one of ${STEP_KINDS.join(", ")}: ${JSON.stringify(step)}`);
  }
  return body as Step[];
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.GATE_TOKEN}`)
      return json({ error: "unauthorized" }, 401);
    if (new URL(req.url).pathname !== "/steps" || req.method !== "POST")
      return json({ error: "not found" }, 404);
    let steps: Step[];
    try {
      steps = parseSteps(await req.json());
    } catch (e) {
      return json({ error: String(e) }, 400);
    }
    const outcomes: Outcome[] = [];
    for (const step of steps) {
      try {
        outcomes.push({ step, ok: true, rows: await runStep(env, step) });
      } catch (e) {
        outcomes.push({ step, ok: false, error: String(e) });
      }
    }
    return json(outcomes);
  },
};
