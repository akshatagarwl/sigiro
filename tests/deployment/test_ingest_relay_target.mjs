import assert from "node:assert/strict";
import test from "node:test";

import {
  relayTarget,
  retryProfileAdmission,
  validPyroscopeContentType,
} from "../../ops/optimizer/relay-target.mjs";

const name =
  "worldpol-pipeline-worker{process_id=123,service_name=worldpol-pipeline-worker}";
const pyroscope = `/ingest?from=1790590000&name=${encodeURIComponent(name)}&spyName=nodespy&until=1790590060&sampleRate=100`;

test("only bounded SDK upload paths can use tenant OAuth", () => {
  assert.deepEqual(relayTarget(pyroscope), {
    path: pyroscope,
    counter: "profiles",
  });
  assert.deepEqual(relayTarget("/v1/logs"), {
    path: "/v1/logs",
    counter: "logs",
  });
  for (const raw of [
    "//evil.example/ingest",
    "/v1/logs?x=1",
    "/ingest",
    "/ingest?name=x",
    `${pyroscope}&redirect=https://evil.example`,
    `${pyroscope}&name=duplicate`,
    `${pyroscope}&from=1`,
    `${pyroscope}#fragment`,
    "/ingest?from=1&until=2&name=x&spyName=nodespy&sampleRate=-1",
    `${pyroscope}&junk=${"x".repeat(1024)}`,
  ])
    assert.equal(relayTarget(raw), null, raw);
});

test("profile admission retries are bounded and do not affect OTLP", async () => {
  let calls = 0;
  const sleeps = [];
  const request = async () => {
    calls++;
    return new Response("", { status: calls < 2 ? 429 : 200 });
  };
  const sleep = async (delay) => {
    sleeps.push(delay);
  };
  let response = await retryProfileAdmission(
    "/ingest?name=worker",
    new Response("", { status: 429 }),
    request,
    sleep
  );
  assert.equal(response.status, 200);
  assert.equal(calls, 2);
  assert.deepEqual(sleeps, [250, 750]);
  response = await retryProfileAdmission(
    "/v1/logs",
    new Response("", { status: 429 }),
    request,
    sleep
  );
  assert.equal(response.status, 429);
  assert.equal(calls, 2);
  assert.deepEqual(sleeps, [250, 750]);
  calls = 0;
  response = await retryProfileAdmission(
    "/ingest?name=worker",
    new Response("", { status: 429 }),
    async () => {
      calls++;
      return new Response("", { status: 429 });
    },
    sleep
  );
  assert.equal(response.status, 429);
  assert.equal(calls, 2);
});

test("Pyroscope multipart content type is required", () => {
  assert.equal(
    validPyroscopeContentType(
      "multipart/form-data; boundary=----formdata-undici-123"
    ),
    true
  );
  assert.equal(
    validPyroscopeContentType(
      'multipart/form-data; boundary="----formdata-undici-123"'
    ),
    true
  );
  for (const value of [
    undefined,
    "application/json",
    "multipart/form-data",
    "multipart/form-data; boundary=a\r\nAuthorization: x",
  ]) {
    assert.equal(validPyroscopeContentType(value), false);
  }
});
