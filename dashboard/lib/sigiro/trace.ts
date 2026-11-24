import { num, query, sqlString, type Row } from "@/lib/sigiro/query";

export type Span = {
  id: string;
  parent: string;
  name: string;
  service: string;
  start: number;
  duration: number;
  error: boolean;
  row: Row;
};
export type SpanNode = { span: Span; depth: number; children: boolean };

export function traceModel(rows: Row[]) {
  const spans: Span[] = rows.map((row) => ({
    id: String(row.span_id),
    parent: String(row.parent_span_id ?? ""),
    name: String(row.span_name ?? ""),
    service: String(row.service_name ?? ""),
    start: num(row.start_us),
    duration: Math.max(0, num(row.duration)),
    error: num(row.status_code) === 2,
    row,
  }));
  const byId = new Map(spans.map((span) => [span.id, span]));
  const children = new Map<string, Span[]>();
  for (const span of spans) {
    const key =
      byId.has(span.parent) && span.parent !== span.id ? span.parent : "";
    const group = children.get(key) ?? [];
    group.push(span);
    children.set(key, group);
  }
  const ordered: SpanNode[] = [];
  const seen = new Set<string>();
  const walk = (root: Span) => {
    const stack = [{ span: root, depth: 0 }];
    while (stack.length) {
      const node = stack.pop()!;
      if (seen.has(node.span.id)) continue;
      seen.add(node.span.id);
      const nested = children.get(node.span.id) ?? [];
      ordered.push({ ...node, children: nested.length > 0 });
      for (let i = nested.length - 1; i >= 0; i--)
        stack.push({ span: nested[i], depth: node.depth + 1 });
    }
  };
  for (const span of children.get("") ?? []) walk(span);
  for (const span of spans) if (!seen.has(span.id)) walk(span);
  const start = Math.min(...spans.map((span) => span.start));
  const end = Math.max(...spans.map((span) => span.start + span.duration));
  return { spans, ordered, byId, start, duration: Math.max(1, end - start) };
}

const DAY_US = 86_400_000_000;

/**
 * Put attribute values of 1 KiB or more, stored once per day in
 * `sigiro_span_values`, back into each row's `span_attributes`. Rows need
 * `start_us`, `span_attributes` and `span_attribute_refs`.
 */
export async function restoreSpanValues(
  rows: Row[],
  signal?: AbortSignal
): Promise<Row[]> {
  const refsOf = (row: Row) =>
    typeof row.span_attribute_refs === "string"
      ? (JSON.parse(row.span_attribute_refs) as Record<string, string>)
      : {};
  const days = new Set<number>();
  const hashes = new Set<string>();
  for (const row of rows) {
    const refs = Object.values(refsOf(row));
    if (refs.length === 0) continue;
    days.add((Math.floor(num(row.start_us) / DAY_US) + 1) * DAY_US - 1);
    for (const hash of refs) hashes.add(hash);
  }
  if (hashes.size === 0) return rows;
  const values = new Map<string, unknown>();
  for (const row of await query(
    `SELECT hash, any_value(value) AS value FROM sigiro_span_values WHERE timestamp IN (${[...days].map((day) => `make_timestamp(${day})`).join(", ")}) AND hash IN (${[...hashes].map(sqlString).join(", ")}) GROUP BY hash`,
    signal
  ))
    values.set(String(row.hash), JSON.parse(String(row.value)));
  return rows.map((row) => {
    const refs = Object.entries(refsOf(row));
    if (refs.length === 0) return row;
    const attributes = parsePayload(row.span_attributes) as Record<
      string,
      unknown
    >;
    for (const [key, hash] of refs)
      if (values.has(hash)) attributes[key] = values.get(hash);
    return { ...row, span_attributes: JSON.stringify(attributes) };
  });
}

export function parsePayload(value: unknown): unknown {
  if (typeof value !== "string") return value ?? {};
  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}

export function durationLabel(micros: number) {
  if (micros < 1000) return `${Math.round(micros)} µs`;
  if (micros < 1_000_000) return `${(micros / 1000).toFixed(2)} ms`;
  return `${(micros / 1_000_000).toFixed(2)} s`;
}
