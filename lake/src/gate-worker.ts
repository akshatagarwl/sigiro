// Runtime-gate Worker: runs an ordered list of steps (SQL, catalog publish, catalog restore)
// inside ONE request, because Workers isolates are not sticky between requests.
// Deployed only by test/runtime-gate-e2e.ts and deleted afterwards.
import { discardIfFatal, fileSystem, lakeConnection } from "./runtime";

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
  const object = await env.LAKE.get(step.restore.key);
  if (!object) throw new Error(`no catalog at ${step.restore.key}`);
  fileSystem().writeFile(
    step.restore.catalog,
    new Uint8Array(await object.arrayBuffer())
  );
  return undefined;
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.GATE_TOKEN}`)
      return json({ error: "unauthorized" }, 401);
    if (new URL(req.url).pathname !== "/steps" || req.method !== "POST")
      return json({ error: "not found" }, 404);
    const steps = (await req.json()) as Step[];
    const outcomes: Outcome[] = [];
    for (const step of steps) {
      try {
        outcomes.push({ step, ok: true, rows: await runStep(env, step) });
      } catch (e) {
        discardIfFatal(e);
        outcomes.push({ step, ok: false, error: String(e) });
      }
    }
    return json(outcomes);
  },
};
