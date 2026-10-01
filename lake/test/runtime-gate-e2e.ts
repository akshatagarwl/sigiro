// E2E for runtime-gate.feature against real Cloudflare Workers.
// Deploys src/gate-worker.ts, runs the three scenarios, and always deletes the Worker.
// Tables come from src/schema.rs and SQL from the English docs: no copies to drift.
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import type { Outcome, Step } from "../src/gate-worker";

const lakeDir = new URL("..", import.meta.url).pathname;
const repo = join(lakeDir, "..");
const bucket = "sigiro-lake-runtime-gate";
const run = `gate-${Date.now()}`;
const token = Buffer.from(crypto.getRandomValues(new Uint8Array(24))).toString("hex");

function wrangler(args: string[], input?: string): string {
  return execFileSync("aube", ["exec", "--", "wrangler", ...args], { cwd: lakeDir, encoding: "utf8", input, stdio: ["pipe", "pipe", "pipe"] });
}

/** Every `CREATE TABLE` literal in src/schema.rs, qualified into the given DuckLake catalog. */
function sigiroTables(catalog: string): string[] {
  const rust = readFileSync(join(repo, "src/schema.rs"), "utf8");
  const ddl = [...rust.matchAll(/"(CREATE TABLE IF NOT EXISTS \{prefix\}[\s\S]*?)"\s*\)\)/g)].map((m) => m[1].replaceAll("{prefix}", `${catalog}.`));
  if (ddl.length !== 7) throw new Error(`expected 7 Sigiro tables in src/schema.rs, found ${ddl.length}`);
  return ddl;
}

/** Every statement in a ```sql block of the English docs (locale directories excluded). */
function documentedSql(): { source: string; sql: string }[] {
  const docs = join(repo, "www/docs");
  const locales = new Set(readdirSync(docs).filter((d) => /^[a-z]{2}$/.test(d)));
  const files: string[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) { if (!(dir === docs && locales.has(name))) walk(path); }
      else if (name.endsWith(".md")) files.push(path);
    }
  };
  walk(docs);
  const out: { source: string; sql: string }[] = [];
  for (const file of files) {
    for (const block of readFileSync(file, "utf8").matchAll(/^```sql\n([\s\S]*?)^```/gm)) {
      for (const sql of block[1].split(/;\s*(?:\n|$)/).map((s) => s.trim()).filter((s) => s.replace(/--.*$/gm, "").trim())) {
        out.push({ source: relative(repo, file), sql });
      }
    }
  }
  if (out.length === 0) throw new Error("no ```sql examples found in www/docs");
  return out;
}

const seed = (catalog: string) => [
  `INSERT INTO ${catalog}.sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id, service_name, span_name, span_kind, status_code, resource_attributes, span_attributes)
   VALUES (now()::TIMESTAMP, 0, 1500, '0af7651916cd43dd8448eb211c80319c', 'b7ad6b7169203331', 'checkout', 'GET /cart', 2, 0, '{"service.version":"1.4.2"}', '{"http.route":"/cart"}')`,
  `INSERT INTO ${catalog}.sigiro_logs (timestamp, observed_timestamp, service_name, severity_number, severity_text, body, log_attributes)
   VALUES (now()::TIMESTAMP, 0, 'claude-code', 17, 'ERROR', 'claude_code.api_request', '{"session.id":"baseline-session-id","cost_usd":"0.12","input_tokens":"10","http.route":"/cart"}')`,
  `INSERT INTO ${catalog}.sigiro_metrics_gauge (timestamp, metric_name, value, service_name)
   VALUES (TIMESTAMP '2026-09-12 12:00:00', 'system.filesystem.usage', 0.5, 'checkout')`,
];

async function steps(url: string, list: Step[]): Promise<Outcome[]> {
  const res = await fetch(`${url}/steps`, { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify(list) });
  if (!res.ok) throw new Error(`gate Worker returned HTTP ${res.status}: ${(await res.text()).slice(0, 500)}`);
  return (await res.json()) as Outcome[];
}

const failures: string[] = [];
const check = (scenario: string, ok: boolean, detail: string) => {
  console.log(`${ok ? "PASS" : "FAIL"}  ${scenario}${ok ? "" : `\n      ${detail}`}`);
  if (!ok) failures.push(scenario);
};

let deployed = false;
try {
  try { wrangler(["r2", "bucket", "create", bucket]); } catch (e) { if (!String((e as { stderr?: string }).stderr).includes("already exists")) throw e; }

  // Scenario: The runtime deploys within the Workers size limit
  let url = "";
  try {
    const out = wrangler(["deploy"]);
    deployed = true;
    url = out.match(/https:\/\/sigiro-lake-runtime-gate\.[\w-]+\.workers\.dev/)?.[0] ?? "";
    wrangler(["secret", "put", "GATE_TOKEN"], token);
    check("The runtime deploys within the Workers size limit", url !== "", `no workers.dev URL in deploy output:\n${out.slice(-800)}`);
  } catch (e) {
    check("The runtime deploys within the Workers size limit", false, String((e as { stderr?: string }).stderr ?? e).slice(-1200));
    throw new Error("deployment failed; remaining scenarios cannot run");
  }
  for (let i = 0; i < 30; i++) { // the secret propagates to the edge asynchronously
    const r = await fetch(`${url}/steps`, { method: "POST", headers: { authorization: `Bearer ${token}` }, body: "[]" });
    if (r.status === 200) break;
    await new Promise((r) => setTimeout(r, 2000));
  }

  // Scenario: Every documented SQL example runs on the Workers runtime
  const examples = documentedSql();
  const attach = `ATTACH 'ducklake:/tmp/${run}.ducklake' AS lake (DATA_PATH 's3://${bucket}/${run}/', METADATA_PARAMETERS MAP {'block_size': 16384})`;
  const setup: Step[] = [attach, ...sigiroTables("lake"), ...seed("lake"), "USE lake"].map((sql) => ({ sql }));
  const outcomes = await steps(url, [...setup, ...examples.map(({ sql }) => ({ sql }))]);
  const setupFailures = outcomes.slice(0, setup.length).filter((o) => !o.ok);
  check("Lake setup from Sigiro's table definitions", setupFailures.length === 0, setupFailures.map((o) => `${"sql" in o.step ? o.step.sql.slice(0, 80) : ""}: ${!o.ok ? o.error.slice(0, 300) : ""}`).join("\n      "));
  const exampleFailures = outcomes.slice(setup.length).map((o, i) => ({ o, e: examples[i] })).filter(({ o }) => !o.ok);
  check(`Every documented SQL example runs on the Workers runtime (${examples.length - exampleFailures.length}/${examples.length})`, exampleFailures.length === 0,
    exampleFailures.map(({ o, e }) => `${e.source}: ${!o.ok ? o.error.split("\n")[0].slice(0, 240) : ""}`).join("\n      "));

  // Scenario: A tenant lake catalog survives a restart
  const key = `${run}/catalog.ducklake`;
  const published = `/tmp/${run}-published.ducklake`;
  const publish = await steps(url, [
    { sql: `ATTACH 'ducklake:${published}' AS published (DATA_PATH 's3://${bucket}/${run}/published/', METADATA_PARAMETERS MAP {'block_size': 16384})` },
    ...sigiroTables("published").map((sql) => ({ sql })),
    { sql: seed("published")[0] },
    { publish: { database: "__ducklake_metadata_published", catalog: published, key } },
  ]);
  const restore = await steps(url, [
    { restore: { key, catalog: `/tmp/${run}-restored.ducklake` } },
    { sql: `ATTACH 'ducklake:/tmp/${run}-restored.ducklake' AS restored (READ_ONLY, DATA_PATH 's3://${bucket}/${run}/published/')` },
    { sql: "SELECT count(*)::INTEGER AS spans FROM restored.sigiro_spans" },
  ]);
  const last = restore.at(-1)!;
  const spans = last.ok ? (last.rows?.[0] as { spans?: number } | undefined)?.spans : undefined;
  const failed = [...publish, ...restore].filter((o) => !o.ok).map((o) => (!o.ok ? o.error.slice(0, 300) : ""));
  check("A tenant lake catalog survives a restart", failed.length === 0 && spans === 1, failed.join("\n      ") || `restored lake returned ${spans} spans`);
} catch (e) {
  if (failures.length === 0) failures.push(String(e));
  console.error(String(e));
} finally {
  if (deployed) wrangler(["delete", "--force"]);
}

if (failures.length) { console.error(`\n${failures.length} scenario(s) failed`); process.exit(1); }
console.log("\nruntime gate passed");
