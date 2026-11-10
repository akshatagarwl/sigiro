import assert from "node:assert/strict";

import {
  Fleet,
  ingestHealth,
  postUntilAccepted,
  token,
  traceBody,
  waitFor,
} from "./harness.ts";

// The last API revision before the ingest cell became an explicit state
// machine: its WAL rows must survive the reshape that adds the row state.
const previous = "c583c035a";
const fleet = new Fleet();
const ingest = token("acme", "telemetry:ingest");
const read = token("acme", "query:read");

try {
  await fleet.start({ FLUSH_SECONDS: "1" });
  fleet.deployApiRevision(previous, { FLUSH_SECONDS: "30" });
  await fleet.restart("SIGTERM");
  await postUntilAccepted(
    "/v1/traces",
    traceBody([{ name: "before-upgrade", service: "checkout" }]),
    ingest,
    "application/x-protobuf"
  );
  assert.equal(
    fleet.count("tenants/acme/spans/", "span_name", "before-upgrade"),
    0,
    "acknowledged but not flushed before the upgrade"
  );

  fleet.deploy({ FLUSH_SECONDS: "1" });
  await fleet.restart("SIGTERM");
  await postUntilAccepted(
    "/v1/traces",
    traceBody([{ name: "after-upgrade", service: "checkout" }]),
    ingest,
    "application/x-protobuf"
  );
  await fleet.waitForRow("tenants/acme/spans/", "span_name", "before-upgrade");
  await fleet.waitForRow("tenants/acme/spans/", "span_name", "after-upgrade");
  assert.equal(
    fleet.count("tenants/acme/spans/", "span_name", "before-upgrade"),
    1,
    "the acknowledged span survives the storage reshape once"
  );
  await waitFor(async () => {
    const cell = (await ingestHealth(read)).ingest.traces;
    return cell.pendingRows === 0 && cell.parked === 0;
  }, "the upgraded cell drains with nothing parked");

  console.log("cloud upgrade e2e passed");
} finally {
  await fleet.stop();
}
