import assert from "node:assert/strict";
import { Readable } from "node:stream";
import { test } from "node:test";

import {
  InvalidBody,
  readBoundedBody,
} from "../../ops/optimizer/read-bounded-body.mjs";

const request = (parts, headers = {}) =>
  Object.assign(Readable.from(parts.map((part) => Buffer.from(part))), {
    headers,
  });

test("chunked OTLP payloads are accepted within the byte cap", async () => {
  const body = await readBoundedBody(
    request(["abc", "def"], { "transfer-encoding": "chunked" }),
    8
  );
  assert.equal(body.toString(), "abcdef");
});

test("explicit content length must match", async () => {
  assert.equal(
    (
      await readBoundedBody(request(["data"], { "content-length": "4" }), 8)
    ).toString(),
    "data"
  );
  await assert.rejects(
    readBoundedBody(request(["data"], { "content-length": "5" }), 8),
    (error) => error instanceof InvalidBody && error.status === 400
  );
  await assert.rejects(
    readBoundedBody(request(["data"], { "content-length": "4x" }), 8),
    (error) => error instanceof InvalidBody && error.status === 400
  );
});

test("chunked and declared bodies cannot exceed the cap", async () => {
  for (const headers of [
    { "transfer-encoding": "chunked" },
    { "content-length": "9" },
  ]) {
    await assert.rejects(
      readBoundedBody(request(["12345", "6789"], headers), 8),
      (error) => error instanceof InvalidBody && error.status === 413
    );
  }
});
