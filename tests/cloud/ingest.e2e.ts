import assert from "node:assert/strict";
import { rmSync } from "node:fs";

import {
  Fleet,
  duckdb,
  expectStatus,
  get,
  gzip,
  ingestHealth,
  metricsBody,
  otlpProfileBody,
  post,
  postUntilAccepted,
  token,
  traceBody,
  waitFor,
} from "./harness.ts";

const fleet = new Fleet();
const ingest = token("acme", "telemetry:ingest");
const protobuf = "application/x-protobuf";

try {
  await fleet.start({
    FLUSH_SECONDS: "1",
    SIGIRO_MAX_INGEST_BODY_BYTES: "1024",
  });

  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "POST /checkout", service: "checkout" }]),
      ingest,
      protobuf
    ),
    202
  );
  await fleet.waitForRow("tenants/acme/spans/", "span_name", "POST /checkout");

  await expectStatus(
    await post(
      "/v1/metrics",
      metricsBody("checkout"),
      ingest,
      "application/json"
    ),
    202
  );
  await fleet.waitForRow(
    "tenants/acme/metrics_gauge/",
    "metric_name",
    "cart.items"
  );
  await fleet.waitForRow(
    "tenants/acme/metrics_sum/",
    "metric_name",
    "checkout.total"
  );

  await expectStatus(
    await post(
      "/v1/traces",
      gzip(traceBody([{ name: "gzipped", service: "checkout" }])),
      ingest,
      protobuf,
      { "content-encoding": "gzip" }
    ),
    202
  );
  await fleet.waitForRow("tenants/acme/spans/", "span_name", "gzipped");

  await expectStatus(
    await post(
      "/v1development/profiles",
      otlpProfileBody("otlp-profiled"),
      ingest,
      protobuf
    ),
    202
  );
  await fleet.waitForRow(
    "tenants/acme/profiles/",
    "service_name",
    "otlp-profiled"
  );
  const profiles = fleet.download(fleet.keys("tenants/acme/profiles/"));
  try {
    assert.equal(
      duckdb(
        `select array_to_string(stack_trace, ';') as stack, value from read_parquet('${profiles}/*.parquet') order by value desc`
      ),
      "stack,value\nmain;work,10\nmain,5"
    );
  } finally {
    rmSync(profiles, { recursive: true, force: true });
  }
  // Only OTLP carries profiles; the old non-OTLP profile paths are gone.
  await expectStatus(
    await post("/ingest", otlpProfileBody("gone"), ingest, protobuf),
    404
  );

  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "path-tenant", service: "globex" }]),
      token("../globex", "telemetry:ingest"),
      protobuf
    ),
    401
  );
  for (const key of fleet.keys("")) {
    assert.equal(key.includes(".."), false, key);
  }

  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "acme-only", service: "acme" }]),
      ingest,
      protobuf
    ),
    202
  );
  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "globex-only", service: "globex" }]),
      token("globex", "telemetry:ingest"),
      protobuf
    ),
    202
  );
  await fleet.waitForRow("tenants/acme/spans/", "span_name", "acme-only");
  await fleet.waitForRow("tenants/globex/spans/", "span_name", "globex-only");
  assert.equal(
    fleet.count("tenants/acme/spans/", "span_name", "globex-only"),
    0
  );
  assert.equal(
    fleet.count("tenants/globex/spans/", "span_name", "acme-only"),
    0
  );

  const before = fleet.keys("tenants/acme/spans/").length;
  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "blocked", service: "checkout" }]),
      token("acme", "query:read"),
      protobuf
    ),
    403
  );
  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "missing", service: "checkout" }]),
      "",
      protobuf
    ),
    401
  );
  await expectStatus(
    await post(
      "/v1/traces",
      traceBody([{ name: "wrong", service: "checkout" }]),
      ingest,
      "text/plain"
    ),
    415
  );
  await expectStatus(
    await post("/v1/traces", new Uint8Array(1_025), ingest, protobuf),
    413
  );
  assert.equal(fleet.keys("tenants/acme/spans/").length, before);

  fleet.deploy({ FLUSH_SECONDS: "15", SIGIRO_MAX_INGEST_BODY_BYTES: "1024" });
  await fleet.restart("SIGTERM");
  await postUntilAccepted(
    "/v1/traces",
    traceBody([{ name: "restart-survivor", service: "checkout" }]),
    ingest,
    protobuf
  );
  assert.equal(
    fleet.count("tenants/acme/spans/", "span_name", "restart-survivor"),
    0,
    "acknowledged before its flush"
  );
  await fleet.restart("SIGKILL");
  await postUntilAccepted(
    "/v1/traces",
    traceBody([{ name: "after-crash", service: "checkout" }]),
    ingest,
    protobuf
  );
  await fleet.waitForRow(
    "tenants/acme/spans/",
    "span_name",
    "restart-survivor"
  );
  assert.equal(
    fleet.count("tenants/acme/spans/", "span_name", "restart-survivor"),
    1,
    "survives restart exactly once"
  );

  // /health: tenant-scoped ingest lag and parked rows, behind a token.
  await expectStatus(await get("/health"), 401);
  await expectStatus(await get("/health", token("acme", "services:read")), 403);
  await waitFor(async () => {
    const health = await ingestHealth(token("acme", "query:read"));
    const cell = health.ingest.traces;
    return (
      health.status === "ready" &&
      cell.pendingRows === 0 &&
      cell.lagMs === 0 &&
      cell.parked === 0 &&
      cell.phase !== "flushing"
    );
  }, "the traces cell drains and reports no lag");
  const health = await ingestHealth(ingest);
  assert.deepEqual(Object.keys(health.ingest).sort(), [
    "logs",
    "metrics",
    "profiles",
    "traces",
  ]);

  // Past FLUSH_BYTES the write still answers 202 straight away; the flush
  // runs from the alarm, never in the request path.
  fleet.deploy({
    FLUSH_SECONDS: "600",
    FLUSH_BYTES: "1",
    SIGIRO_MAX_INGEST_BODY_BYTES: "1024",
  });
  await fleet.restart("SIGTERM");
  await postUntilAccepted(
    "/v1/traces",
    traceBody([{ name: "over-flush-bytes", service: "checkout" }]),
    ingest,
    protobuf
  );
  const pending = (await ingestHealth(ingest)).ingest.traces;
  assert.ok(pending.pendingRows <= 1, JSON.stringify(pending));
  await fleet.waitForRow(
    "tenants/acme/spans/",
    "span_name",
    "over-flush-bytes"
  );
  assert.equal(
    fleet.count("tenants/acme/spans/", "span_name", "over-flush-bytes"),
    1,
    "flushed once from the alarm, well before FLUSH_SECONDS"
  );

  console.log("cloud ingest e2e passed");
} finally {
  await fleet.stop();
}
