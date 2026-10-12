import assert from "node:assert/strict";
import { rmSync } from "node:fs";
import { Fleet, duckdb, expectStatus, logsBody, origin, otlpProfileBody, post, token, traceBody } from "./harness.ts";

const fleet = new Fleet();
const protobuf = "application/x-protobuf";

async function query(tenant: string, sql: string, scope = "query:read"): Promise<Response> {
  return post("/v1/query", sql, token(tenant, scope), "text/plain");
}

try {
  await fleet.start({ FLUSH_SECONDS: "1" });
  const acme = token("acme", "telemetry:ingest");
  await expectStatus(await post("/v1/traces", traceBody([{ name: "POST /checkout", service: "checkout" }, { name: "POST /checkout", service: "checkout" }, { name: "GET /cart", service: "checkout" }]), acme, protobuf), 202);
  await expectStatus(await post("/v1/logs", logsBody("worker", "card declined"), acme, "application/json"), 202);
  await expectStatus(await post("/v1development/profiles", otlpProfileBody("profiler"), acme, protobuf), 202);
  await expectStatus(await post("/v1/traces", traceBody([{ name: "globex-only", service: "billing" }]), token("globex", "telemetry:ingest"), protobuf), 202);
  await fleet.waitForRow("tenants/acme/spans/", "span_name", "GET /cart");
  await fleet.waitForRow("tenants/acme/logs/", "service_name", "worker");
  await fleet.waitForRow("tenants/globex/spans/", "span_name", "globex-only");
  await fleet.waitForRow("tenants/acme/profiles/", "service_name", "profiler");

  const grouped = await query("acme", "SELECT span_name, count(*) AS n FROM sigiro_spans GROUP BY 1 ORDER BY 1");
  const groupedBody = await expectStatus(grouped, 200);
  assert.equal(grouped.headers.get("x-sigiro-truncated"), "false");
  assert.deepEqual(JSON.parse(groupedBody), [{ span_name: "GET /cart", n: 1 }, { span_name: "POST /checkout", n: 2 }]);

  const parity = "SELECT service_name, count(*) AS n FROM sigiro_spans GROUP BY 1 ORDER BY 1";
  const hosted = JSON.parse(await expectStatus(await query("acme", parity), 200)) as { service_name: string; n: number }[];
  const dir = fleet.download(fleet.keys("tenants/acme/spans/"));
  try {
    const cli = duckdb(parity.replace("sigiro_spans", `read_parquet('${dir}/*.parquet')`));
    assert.equal(["service_name,n", ...hosted.map((row) => `${row.service_name},${row.n}`)].join("\n"), cli);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }

  assert.deepEqual(JSON.parse(await expectStatus(await query("globex", "SELECT span_name FROM sigiro_spans"), 200)), [{ span_name: "globex-only" }]);

  for (const sql of ["DELETE FROM sigiro_spans", "ATTACH 'x.db' AS x", "SELECT * FROM read_parquet('r2://x/y')", "SELECT table_name FROM information_schema.tables"]) {
    assert.equal(JSON.parse(await expectStatus(await query("acme", sql), 400)).title, "query rejected", sql);
  }

  await expectStatus(await query("acme", "SELECT 1 AS one FROM sigiro_spans", "telemetry:ingest"), 403);

  const exhausted = JSON.parse(await expectStatus(await query("acme", "SELECT median(i) AS m FROM range(100000000) t(i)"), 400)) as { title: string; detail: string };
  assert.equal(exhausted.title, "query rejected");
  assert.match(exhausted.detail, /memory limit/u);
  assert.deepEqual(JSON.parse(await expectStatus(await query("acme", "SELECT count(*) AS n FROM sigiro_spans"), 200)), [{ n: 3 }]);

  const services = await fetch(`${origin}/v1/services`, { headers: { authorization: `Bearer ${token("acme", "services:read")}` } });
  const body = JSON.parse(await expectStatus(services, 200)) as { services: { name: string; totalCount: number; signals: Record<string, number> }[] };
  assert.deepEqual(body.services.map(({ name, totalCount, signals }) => ({ name, totalCount, signals })), [
    { name: "checkout", totalCount: 3, signals: { spans: 3, logs: 0, metrics: 0, profiles: 0 } },
    { name: "profiler", totalCount: 2, signals: { spans: 0, logs: 0, metrics: 0, profiles: 2 } },
    { name: "worker", totalCount: 1, signals: { spans: 0, logs: 1, metrics: 0, profiles: 0 } },
  ]);

  assert.deepEqual(JSON.parse(await expectStatus(await query("acme", "SELECT service_name, sum(value) AS total FROM sigiro_profiles GROUP BY 1"), 200)), [{ service_name: "profiler", total: 15 }]);

  console.log("cloud query e2e passed");
} finally {
  await fleet.stop();
}
