import assert from "node:assert/strict";
import test from "node:test";

import { authUIResponse } from "../src/ui.js";

void test("serves the Agent Auth device approval page", () => {
  const response = authUIResponse("/device/capabilities");

  if (!response) throw new Error("approval page was not served");
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html/);
  assert.match(response.headers.get("cache-control") ?? "", /no-store/);
  return response.text().then((body) => {
    assert.match(body, /data-page="agent-device"/);
    assert.match(body, /Connect an agent to Sigiro/);
    assert.match(body, /auth-ui\.js/);
  });
});

void test("does not claim unrelated routes", () => {
  assert.equal(authUIResponse("/device/capabilities/unknown"), undefined);
});
