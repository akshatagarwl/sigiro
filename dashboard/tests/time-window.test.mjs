import assert from "node:assert/strict";
import test from "node:test";

import { resolveTimeWindow } from "../lib/sigiro/time-window.ts";

const now = Date.UTC(2026, 10, 13, 12);
test("arbitrary absolute windows preserve millisecond endpoints", () => {
  const result = resolveTimeWindow("1700000000123", "1700003600456", now);
  assert.equal(result.from.getTime(), 1700000000123);
  assert.equal(result.to.getTime(), 1700003600456);
  assert.equal(result.start.type, "absolute");
});
test("relative windows use one anchor and advance only when it changes", () => {
  const first = resolveTimeWindow("now-37m", "now", now);
  const next = resolveTimeWindow("now-37m", "now", now + 10_000);
  assert.equal(first.to.getTime() - first.from.getTime(), 37 * 60_000);
  assert.equal(next.from.getTime() - first.from.getTime(), 10_000);
  assert.equal(next.to.getTime() - first.to.getTime(), 10_000);
  const offset = resolveTimeWindow("now-2h", "now-1h", now);
  assert.equal(offset.to.getTime(), now - 3_600_000);
});
test("months and years resolve as calendar units", () => {
  assert.equal(
    resolveTimeWindow("now-1M", "now", now).from.toISOString(),
    "2026-10-13T12:00:00.000Z"
  );
  assert.equal(
    resolveTimeWindow("now-1y", "now", now).from.toISOString(),
    "2025-11-13T12:00:00.000Z"
  );
});
test("invalid, reversed, equal and overflowing windows fail closed", () => {
  for (const [start, end] of [
    ["garbage", "now"],
    ["123junk", "now"],
    ["now-1.5h", "now"],
    ["now", "now"],
    ["20", "10"],
    ["now-1h", "now-2h"],
    ["now-9999999999999999999y", "now"],
    ["8640000000000001", "now"],
  ])
    assert.throws(
      () => resolveTimeWindow(start, end, now),
      `${start} / ${end}`
    );
});
