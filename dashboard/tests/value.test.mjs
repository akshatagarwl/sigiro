import assert from "node:assert/strict";
import { test } from "node:test";

import { jsonText, serializeJson } from "../components/observability/value.ts";

test("serialization failures are explicit rather than crashing rendering", () => {
  let deep = {};
  for (let i = 0; i < 20_000; i++) deep = { child: deep };
  const result = serializeJson(deep);
  assert.equal(result.text, "");
  assert.match(result.error, /could not be serialized/);
});

test("preserves full oversized data and JSON scalar types", () => {
  const value = {
    text: "payload".repeat(150_000),
    nil: null,
    boolean: false,
    number: 1,
    array: ["a", 2],
  };
  assert.deepEqual(JSON.parse(jsonText(value)), value);
});

test("represents actual cycles and BigInt without truncating or losing repeated references", () => {
  const shared = { value: "preserved twice" };
  const value = {
    first: shared,
    second: shared,
    big: 123456789012345678901234567890n,
  };
  value.self = value;
  const result = JSON.parse(jsonText(value));
  assert.deepEqual(result.first, shared);
  assert.deepEqual(result.second, shared);
  assert.equal(result.big, "123456789012345678901234567890");
  assert.equal(result.self, "[Circular]");
});
