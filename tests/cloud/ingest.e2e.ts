import assert from "node:assert/strict";
import { rmSync } from "node:fs";

import {
  Fleet,
  duckdb,
  expectStatus,
  gzip,
  metricsBody,
  otlpProfileBody,
  post,
  postUntilAccepted,
  token,
  traceBody,
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

  console.log("cloud ingest e2e passed");
} finally {
  await fleet.stop();
}
