import { num, type Row } from "@/lib/sigiro/query";

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
