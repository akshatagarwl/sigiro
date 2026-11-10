export type CellState =
  | { phase: "idle" }
  | { phase: "scheduled"; at: number }
  | { phase: "flushing"; dirty: boolean };

export type CellEvent =
  | { type: "write"; now: number; due: boolean }
  | { type: "alarm"; now: number }
  | { type: "flushed"; next: number | null };

export type CellDecision = {
  state: CellState;
  flush: boolean;
  alarmAt: number | null;
};

export function cellStep(
  state: CellState,
  event: CellEvent,
  flushDelayMs: number
): CellDecision {
  const stay = { state, flush: false, alarmAt: null };
  switch (event.type) {
    case "write": {
      if (state.phase === "flushing") {
        return { ...stay, state: { phase: "flushing", dirty: true } };
      }
      const want = event.due ? event.now : event.now + flushDelayMs;
      if (state.phase === "scheduled" && state.at <= want) {
        return stay;
      }
      return {
        state: { phase: "scheduled", at: want },
        flush: false,
        alarmAt: want,
      };
    }
    case "alarm":
      if (state.phase === "flushing") {
        return { ...stay, state: { phase: "flushing", dirty: true } };
      }
      return {
        state: { phase: "flushing", dirty: false },
        flush: true,
        alarmAt: null,
      };
    case "flushed":
      if (state.phase !== "flushing") {
        return stay;
      }
      if (state.dirty) {
        return {
          state: { phase: "flushing", dirty: false },
          flush: true,
          alarmAt: null,
        };
      }
      return event.next === null
        ? { state: { phase: "idle" }, flush: false, alarmAt: null }
        : {
            state: { phase: "scheduled", at: event.next },
            flush: false,
            alarmAt: event.next,
          };
  }
}

export const MAX_ROW_ATTEMPTS = 12;
const BASE_BACKOFF_MS = 5_000;
const MAX_BACKOFF_MS = 600_000;

export type RowFailure =
  | { kind: "poison"; reason: string }
  | { kind: "transient"; reason: string };

export type RowOutcome =
  | { kind: "backoff"; attempts: number; nextAt: number; reason: string }
  | { kind: "parked"; reason: string };

export function rowStep(
  attempts: number,
  failure: RowFailure,
  now: number
): RowOutcome {
  if (failure.kind === "poison") {
    return { kind: "parked", reason: failure.reason };
  }
  const next = attempts + 1;
  if (next >= MAX_ROW_ATTEMPTS) {
    return {
      kind: "parked",
      reason: `gave up after ${next} attempts: ${failure.reason}`,
    };
  }
  return {
    kind: "backoff",
    attempts: next,
    nextAt: now + Math.min(BASE_BACKOFF_MS * 2 ** attempts, MAX_BACKOFF_MS),
    reason: failure.reason,
  };
}

export type PendingRow = {
  id: number;
  attempts: number;
};

export type Segment<Table extends string, Metadata> = {
  key: string;
  table: Table;
  metadata: Metadata;
  parquet: Uint8Array;
};

export type FlushPort<
  Row extends PendingRow,
  Table extends string,
  Metadata,
> = {
  now(): number;
  dueRows(now: number): Row[];
  decode(row: Row): Segment<Table, Metadata>[];
  readManifest(): string;
  writeManifest(manifest: string): void;
  registerPending(manifest: string, key: string, table: Table): string;
  commitLive(manifest: string, key: string, metadata: Metadata): string;
  put(key: string, parquet: Uint8Array): Promise<void>;
  sync(): Promise<void>;
  retire(id: number): void;
  backoff(id: number, attempts: number, nextAt: number, reason: string): void;
  park(id: number, reason: string, now: number): void;
  nextDue(): number | null;
};

export type FlushReport = {
  retired: number;
  backoff: number;
  parked: number;
  next: number | null;
};

export async function flushPass<
  Row extends PendingRow,
  Table extends string,
  Metadata,
>(port: FlushPort<Row, Table, Metadata>): Promise<FlushReport> {
  const report: FlushReport = { retired: 0, backoff: 0, parked: 0, next: null };
  for (const row of port.dueRows(port.now())) {
    let segments: Segment<Table, Metadata>[];
    try {
      segments = port.decode(row);
    } catch (error) {
      await fail(port, row, { kind: "poison", reason: message(error) }, report);
      continue;
    }
    try {
      for (const segment of segments) {
        port.writeManifest(
          port.registerPending(port.readManifest(), segment.key, segment.table)
        );
        await port.sync();
        await port.put(segment.key, segment.parquet);
        port.writeManifest(
          port.commitLive(port.readManifest(), segment.key, segment.metadata)
        );
        await port.sync();
      }
      port.retire(row.id);
      await port.sync();
      report.retired += 1;
    } catch (error) {
      await fail(
        port,
        row,
        { kind: "transient", reason: message(error) },
        report
      );
    }
  }
  report.next = port.nextDue();
  return report;
}

async function fail<Row extends PendingRow, Table extends string, Metadata>(
  port: FlushPort<Row, Table, Metadata>,
  row: Row,
  failure: RowFailure,
  report: FlushReport
): Promise<void> {
  const now = port.now();
  const outcome = rowStep(row.attempts, failure, now);
  if (outcome.kind === "parked") {
    console.warn(`parking wal row ${row.id}: ${outcome.reason}`);
    port.park(row.id, outcome.reason, now);
    report.parked += 1;
  } else {
    console.warn(
      `wal row ${row.id} attempt ${outcome.attempts} failed: ${outcome.reason}`
    );
    port.backoff(row.id, outcome.attempts, outcome.nextAt, outcome.reason);
    report.backoff += 1;
  }
  await port.sync();
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
