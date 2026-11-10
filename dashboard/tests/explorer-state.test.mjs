import assert from "node:assert/strict";
import { test } from "node:test";

import {
  decodeKeyset,
  encodeKeyset,
  explorerView,
} from "../lib/sigiro/explorer-state.ts";

const boom = new Error("boom");
const base = {
  rows: 0,
  status: "success",
  error: null,
  isFetching: false,
  isFetchingNextPage: false,
  isFetchNextPageError: false,
  hasMore: false,
};
const at = (patch) => explorerView({ ...base, ...patch });
const rows = (patch) => at({ rows: 50, ...patch });

test("every explorer results state follows the table", () => {
  const cases = [
    // First load.
    [{ status: "pending", isFetching: true }, { kind: "loading" }],
    [{ status: "pending" }, { kind: "loading" }],
    [
      { status: "error", error: boom },
      { kind: "failed", error: boom, retrying: false },
    ],
    [
      { status: "error", error: boom, isFetching: true },
      { kind: "failed", error: boom, retrying: true },
    ],
    [{}, { kind: "empty", refreshing: false }],
    [{ isFetching: true }, { kind: "empty", refreshing: true }],
    // Rows: the load-more control.
    [
      { rows: 50, hasMore: true },
      {
        kind: "rows",
        more: "idle",
        refreshing: false,
        refreshError: null,
        moreError: null,
      },
    ],
    [
      { rows: 50 },
      {
        kind: "rows",
        more: "end",
        refreshing: false,
        refreshError: null,
        moreError: null,
      },
    ],
    [
      { rows: 50, hasMore: true, isFetching: true, isFetchingNextPage: true },
      {
        kind: "rows",
        more: "loading",
        refreshing: false,
        refreshError: null,
        moreError: null,
      },
    ],
    [
      {
        rows: 50,
        hasMore: true,
        status: "error",
        error: boom,
        isFetchNextPageError: true,
      },
      {
        kind: "rows",
        more: "failed",
        refreshing: false,
        refreshError: null,
        moreError: boom,
      },
    ],
    [
      // Retrying a failed page is loading again, not failed.
      {
        rows: 50,
        hasMore: true,
        status: "error",
        error: boom,
        isFetchNextPageError: true,
        isFetching: true,
        isFetchingNextPage: true,
      },
      {
        kind: "rows",
        more: "loading",
        refreshing: false,
        refreshError: null,
        moreError: boom,
      },
    ],
    // Rows: refresh and live ticks never touch the load-more control.
    [
      { rows: 50, hasMore: true, isFetching: true },
      {
        kind: "rows",
        more: "idle",
        refreshing: true,
        refreshError: null,
        moreError: null,
      },
    ],
    [
      { rows: 50, isFetching: true },
      {
        kind: "rows",
        more: "end",
        refreshing: true,
        refreshError: null,
        moreError: null,
      },
    ],
    [
      { rows: 50, hasMore: true, status: "error", error: boom },
      {
        kind: "rows",
        more: "idle",
        refreshing: false,
        refreshError: boom,
        moreError: null,
      },
    ],
    [
      {
        rows: 50,
        hasMore: true,
        status: "error",
        error: boom,
        isFetching: true,
      },
      {
        kind: "rows",
        more: "idle",
        refreshing: true,
        refreshError: boom,
        moreError: null,
      },
    ],
  ];
  for (const [patch, expected] of cases) {
    assert.deepEqual(at(patch), expected, JSON.stringify(patch));
  }
});

test("a refresh tick keeps the load-more control exactly as it was", () => {
  for (const hasMore of [true, false]) {
    const before = rows({ hasMore });
    const during = rows({ hasMore, isFetching: true });
    assert.equal(during.more, before.more);
  }
});

test("a failed page stays at the bottom and never becomes the table error", () => {
  const view = rows({
    hasMore: true,
    status: "error",
    error: boom,
    isFetchNextPageError: true,
  });
  assert.equal(view.refreshError, null);
  assert.equal(view.more, "failed");
});

test("keyset cursors round-trip and reject malformed input", () => {
  for (const keyset of [
    { micros: 1794243364481000, key: "span~with~tildes" },
    { micros: 0, key: "" },
    { micros: 1, key: "a|b|c" },
  ]) {
    assert.deepEqual(decodeKeyset(encodeKeyset(keyset)), keyset);
  }
  for (const bad of [
    null,
    undefined,
    5,
    "",
    "~x",
    "abc~x",
    "1.5~x",
    "99999999999999999999~x",
  ]) {
    assert.equal(decodeKeyset(bad), null, String(bad));
  }
});
