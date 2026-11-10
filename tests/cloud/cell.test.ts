import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type CellDecision,
  type CellEvent,
  type CellState,
  cellStep,
  type FlushPort,
  flushPass,
  MAX_ROW_ATTEMPTS,
  rowStep,
} from "../../cloud/src/cell.ts";

const DELAY = 60_000;
const NOW = 1_000_000;

test("every cell transition", () => {
  const idle: CellState = { phase: "idle" };
  const scheduledLate: CellState = { phase: "scheduled", at: NOW + 120_000 };
  const scheduledSoon: CellState = { phase: "scheduled", at: NOW + 10 };
  const flushing: CellState = { phase: "flushing", dirty: false };
  const dirty: CellState = { phase: "flushing", dirty: true };
  const write = (due: boolean): CellEvent => ({ type: "write", now: NOW, due });
  const alarm: CellEvent = { type: "alarm", now: NOW };
  const flushed = (next: number | null): CellEvent => ({
    type: "flushed",
    next,
  });
  const cases: [string, CellState, CellEvent, CellDecision][] = [
    // Idle
    [
      "idle + write → scheduled after the flush delay",
      idle,
      write(false),
      {
        state: { phase: "scheduled", at: NOW + DELAY },
        flush: false,
        alarmAt: NOW + DELAY,
      },
    ],
    [
      "idle + write past FLUSH_BYTES → alarm now, still not inline",
      idle,
      write(true),
      { state: { phase: "scheduled", at: NOW }, flush: false, alarmAt: NOW },
    ],
    [
      "idle + alarm (alarm outlived eviction) → flushing",
      idle,
      alarm,
      { state: flushing, flush: true, alarmAt: null },
    ],
    ["idle + flushed (stray) → unchanged", idle, flushed(null), stay(idle)],
    // Scheduled
    [
      "scheduled sooner + write → unchanged",
      scheduledSoon,
      write(false),
      stay(scheduledSoon),
    ],
    [
      "scheduled later (a backoff) + write → pulled in to the flush delay",
      scheduledLate,
      write(false),
      {
        state: { phase: "scheduled", at: NOW + DELAY },
        flush: false,
        alarmAt: NOW + DELAY,
      },
    ],
    [
      "scheduled + write past FLUSH_BYTES → alarm now",
      scheduledSoon,
      write(true),
      { state: { phase: "scheduled", at: NOW }, flush: false, alarmAt: NOW },
    ],
    [
      "scheduled + alarm → flushing",
      scheduledSoon,
      alarm,
      { state: flushing, flush: true, alarmAt: null },
    ],
    [
      "scheduled + flushed (stray) → unchanged",
      scheduledSoon,
      flushed(NOW),
      stay(scheduledSoon),
    ],
    // Flushing: single-flight
    [
      "flushing + write → dirty, no second flush",
      flushing,
      write(false),
      stay(dirty),
    ],
    [
      "flushing + write past FLUSH_BYTES → dirty, no second flush",
      flushing,
      write(true),
      stay(dirty),
    ],
    [
      "flushing + alarm → dirty, joins the running flush",
      flushing,
      alarm,
      stay(dirty),
    ],
    [
      "flushing + flushed, nothing pending → idle",
      flushing,
      flushed(null),
      { state: idle, flush: false, alarmAt: null },
    ],
    [
      "flushing + flushed, rows in backoff → scheduled at the earliest",
      flushing,
      flushed(NOW + 5_000),
      {
        state: { phase: "scheduled", at: NOW + 5_000 },
        flush: false,
        alarmAt: NOW + 5_000,
      },
    ],
    // Flushing + dirty: loop once more
    ["dirty + write → dirty", dirty, write(false), stay(dirty)],
    ["dirty + alarm → dirty", dirty, alarm, stay(dirty)],
    [
      "dirty + flushed → flush again, clean",
      dirty,
      flushed(null),
      { state: flushing, flush: true, alarmAt: null },
    ],
    [
      "dirty + flushed with backoff → flush again before scheduling",
      dirty,
      flushed(NOW + 5_000),
      { state: flushing, flush: true, alarmAt: null },
    ],
  ];
  for (const [name, state, event, expected] of cases) {
    assert.deepEqual(cellStep(state, event, DELAY), expected, name);
  }
});

function stay(state: CellState): CellDecision {
  return { state, flush: false, alarmAt: null };
}

test("every row failure transition", () => {
  const poison = { kind: "poison", reason: "bad protobuf" } as const;
  const transient = { kind: "transient", reason: "r2 503" } as const;
  assert.deepEqual(rowStep(0, poison, NOW), {
    kind: "parked",
    reason: "bad protobuf",
  });
  assert.deepEqual(rowStep(5, poison, NOW), {
    kind: "parked",
    reason: "bad protobuf",
  });
  assert.deepEqual(rowStep(0, transient, NOW), {
    kind: "backoff",
    attempts: 1,
    nextAt: NOW + 5_000,
    reason: "r2 503",
  });
  assert.deepEqual(rowStep(3, transient, NOW), {
    kind: "backoff",
    attempts: 4,
    nextAt: NOW + 40_000,
    reason: "r2 503",
  });
  assert.deepEqual(rowStep(MAX_ROW_ATTEMPTS - 2, transient, NOW), {
    kind: "backoff",
    attempts: MAX_ROW_ATTEMPTS - 1,
    nextAt: NOW + 600_000,
    reason: "r2 503",
  });
  assert.deepEqual(rowStep(MAX_ROW_ATTEMPTS - 1, transient, NOW), {
    kind: "parked",
    reason: `gave up after ${MAX_ROW_ATTEMPTS} attempts: r2 503`,
  });
});

type Row = {
  id: number;
  attempts: number;
  body: string;
  nextAt: number;
  parked?: string;
};

/** An in-memory cell: WAL rows, a manifest, an object store with faults. */
class FakeCell implements FlushPort<Row, "spans", { rows: number }> {
  rows = new Map<number, Row>();
  manifest = {
    pending: {} as Record<string, string>,
    live: {} as Record<string, string>,
  };
  objects = new Map<string, Uint8Array>();
  puts: string[] = [];
  failPuts = new Set<string>();
  clock = NOW;

  add(id: number, body: string): void {
    this.rows.set(id, { id, attempts: 0, body, nextAt: 0 });
  }
  now = () => this.clock;
  dueRows = (now: number) =>
    [...this.rows.values()]
      .filter((row) => row.parked === undefined && row.nextAt <= now)
      .sort((a, b) => a.id - b.id)
      .map((row) => ({ ...row }));
  decode = (row: Row) => {
    if (row.body.startsWith("poison")) {
      throw new Error("cannot decode");
    }
    return row.body.split(",").map((part, index) => ({
      key: `${row.id}-${index}`,
      table: "spans" as const,
      metadata: { rows: part.length },
      parquet: new TextEncoder().encode(part),
    }));
  };
  readManifest = () => JSON.stringify(this.manifest);
  writeManifest = (manifest: string) => {
    this.manifest = JSON.parse(manifest);
  };
  registerPending = (manifest: string, key: string, table: "spans") => {
    const parsed = JSON.parse(manifest);
    if (parsed.live[key] === undefined) {
      parsed.pending[key] = table;
    }
    return JSON.stringify(parsed);
  };
  commitLive = (manifest: string, key: string) => {
    const parsed = JSON.parse(manifest);
    delete parsed.pending[key];
    parsed.live[key] = "spans";
    return JSON.stringify(parsed);
  };
  put = async (key: string, parquet: Uint8Array) => {
    this.puts.push(key);
    if (this.failPuts.has(key)) {
      throw new Error(`put ${key} failed`);
    }
    this.objects.set(key, parquet);
  };
  sync = async () => {};
  retire = (id: number) => void this.rows.delete(id);
  backoff = (id: number, attempts: number, nextAt: number) => {
    const row = this.rows.get(id)!;
    row.attempts = attempts;
    row.nextAt = nextAt;
  };
  park = (id: number, reason: string) => {
    this.rows.get(id)!.parked = reason;
  };
  nextDue = () => {
    const pending = [...this.rows.values()].filter(
      (row) => row.parked === undefined
    );
    return pending.length === 0
      ? null
      : Math.min(...pending.map((row) => row.nextAt));
  };
}

test("a clean pass retires every row and commits every segment", async () => {
  const cell = new FakeCell();
  cell.add(1, "a,b");
  cell.add(2, "c");
  const report = await flushPass(cell);
  assert.deepEqual(report, { retired: 2, backoff: 0, parked: 0, next: null });
  assert.deepEqual(Object.keys(cell.manifest.live).sort(), [
    "1-0",
    "1-1",
    "2-0",
  ]);
  assert.deepEqual(cell.manifest.pending, {});
  assert.equal(cell.rows.size, 0);
});

test("a poison row parks and the rows after it still flush", async () => {
  const cell = new FakeCell();
  cell.add(1, "a");
  cell.add(2, "poison");
  cell.add(3, "c");
  const report = await flushPass(cell);
  assert.deepEqual(report, { retired: 2, backoff: 0, parked: 1, next: null });
  assert.equal(cell.rows.get(2)?.parked, "cannot decode");
  assert.deepEqual(Object.keys(cell.manifest.live).sort(), ["1-0", "3-0"]);
  // Parked rows are skipped from then on.
  assert.deepEqual(await flushPass(cell), {
    retired: 0,
    backoff: 0,
    parked: 0,
    next: null,
  });
});

test("an R2 failure backs the row off, and the retry does not duplicate", async () => {
  const cell = new FakeCell();
  cell.add(1, "a,b");
  cell.add(2, "c");
  cell.failPuts.add("1-1");
  const first = await flushPass(cell);
  assert.deepEqual(first, {
    retired: 1,
    backoff: 1,
    parked: 0,
    next: NOW + 5_000,
  });
  // Segment 1-0 is live, 1-1 is registered but not uploaded.
  assert.deepEqual(cell.manifest.live, { "1-0": "spans", "2-0": "spans" });
  assert.deepEqual(cell.manifest.pending, { "1-1": "spans" });
  // Not due yet: the next pass leaves it alone.
  assert.equal((await flushPass(cell)).backoff, 0);
  cell.failPuts.clear();
  cell.clock = NOW + 5_000;
  assert.deepEqual(await flushPass(cell), {
    retired: 1,
    backoff: 0,
    parked: 0,
    next: null,
  });
  // Same keys, so the retried segment overwrote rather than added a copy.
  assert.deepEqual(Object.keys(cell.manifest.live).sort(), [
    "1-0",
    "1-1",
    "2-0",
  ]);
  assert.deepEqual(cell.manifest.pending, {});
  assert.equal(cell.objects.size, 3);
});

test("a row that keeps failing parks after MAX_ROW_ATTEMPTS", async () => {
  const cell = new FakeCell();
  cell.add(1, "a");
  cell.failPuts.add("1-0");
  for (let attempt = 1; attempt < MAX_ROW_ATTEMPTS; attempt += 1) {
    const report = await flushPass(cell);
    assert.equal(report.backoff, 1, `attempt ${attempt}`);
    cell.clock = report.next!;
  }
  const last = await flushPass(cell);
  assert.deepEqual(last, { retired: 0, backoff: 0, parked: 1, next: null });
  assert.match(cell.rows.get(1)!.parked!, /gave up after 12 attempts/);
  assert.equal(cell.puts.length, MAX_ROW_ATTEMPTS);
});

test("each change re-reads the manifest, so a concurrent commit survives", async () => {
  const cell = new FakeCell();
  cell.add(1, "a");
  const put = cell.put;
  cell.put = async (key, parquet) => {
    // Another writer commits while this upload is in flight.
    cell.manifest.live["other"] = "spans";
    await put(key, parquet);
  };
  await flushPass(cell);
  assert.deepEqual(Object.keys(cell.manifest.live).sort(), ["1-0", "other"]);
});
