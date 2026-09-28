import assert from "node:assert/strict";
import test from "node:test";
import { OPTIMIZER_CLIENT_ID, OPTIMIZER_CLIENT_METADATA_JSON } from "../src/optimizer-client.js";
import { fetchClientMetadataResource } from "../src/d1/cimd-fetch.js";

test("optimizer operator metadata is a dedicated, consented native client", async () => {
  const document = JSON.parse(OPTIMIZER_CLIENT_METADATA_JSON);
  assert.equal(document.client_id, OPTIMIZER_CLIENT_ID);
  assert.equal(new URL(OPTIMIZER_CLIENT_ID).pathname, "/api/auth/optimizer-client.json");
  assert.ok(document.scope.split(" ").includes("tenant:admin"));
  assert.ok(document.scope.split(" ").includes("optimizer:write"));
  const response = await fetchClientMetadataResource(OPTIMIZER_CLIENT_ID, {});
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), document);
});
