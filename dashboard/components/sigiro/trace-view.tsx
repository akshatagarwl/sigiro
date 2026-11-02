"use client";

import { useQuery } from "@tanstack/react-query";

import {
  TraceWaterfall,
  type TraceSpan,
} from "@/components/assistant-ui/elements/trace-waterfall";
import { Skeleton } from "@/components/ui/skeleton";
import { num, query, sqlString } from "@/lib/sigiro/query";

/**
 * One trace as assistant-ui's TraceWaterfall: the spans come from the server
 * (never a sample), depth follows `parent_span_id`, times are relative to the
 * trace's first span.
 */
export function TraceView({ traceId }: { traceId: string }) {
  const valid = /^[0-9a-f]{32}$/i.test(traceId);
  const { data, error, isLoading } = useQuery({
    queryKey: ["trace", traceId],
    enabled: valid,
    queryFn: ({ signal }) =>
      query(
        `SELECT epoch_us(timestamp) AS start_us, duration, service_name, span_name, span_id, parent_span_id, status_code FROM sigiro_spans WHERE trace_id = ${sqlString(traceId.toLowerCase())} ORDER BY timestamp LIMIT 2000`,
        signal
      ),
  });

  if (!valid)
    return <p className="text-destructive text-sm">Not a trace id.</p>;
  if (error)
    return (
      <p className="text-destructive text-sm" role="alert">
        {error.message}
      </p>
    );
  if (isLoading || !data) return <Skeleton className="h-40 w-full" />;
  if (data.length === 0)
    return (
      <p className="text-muted-foreground text-sm">
        No spans stored for this trace.
      </p>
    );

  const start = Math.min(...data.map((row) => num(row.start_us)));
  const end = Math.max(
    ...data.map((row) => num(row.start_us) + num(row.duration))
  );
  const parent = new Map(
    data.map((row) => [String(row.span_id), String(row.parent_span_id ?? "")])
  );
  const depth = (id: string): number => {
    let level = 0;
    for (
      let up = parent.get(id);
      up && parent.has(up) && level < 64;
      up = parent.get(up)
    )
      level++;
    return level;
  };
  // Depth-first by parent, so children sit under the span that ran them.
  const children = new Map<string, typeof data>();
  for (const row of data) {
    const up = String(row.parent_span_id ?? "");
    const key = parent.has(up) ? up : "";
    children.set(key, [...(children.get(key) ?? []), row]);
  }
  const ordered: typeof data = [];
  const walk = (key: string) => {
    for (const row of children.get(key) ?? []) {
      ordered.push(row);
      walk(String(row.span_id));
    }
  };
  walk("");

  const spans: TraceSpan[] = ordered.map((row) => ({
    id: String(row.span_id),
    name: `${row.span_name} · ${row.service_name}`,
    depth: depth(String(row.span_id)),
    startMs: Math.round((num(row.start_us) - start) / 1000),
    durationMs: Math.round(num(row.duration) / 1000),
    status: num(row.status_code) === 2 ? "failed" : "completed",
  }));

  return (
    <div data-waterfall={spans.length}>
      <TraceWaterfall
        spans={spans}
        totalMs={Math.round((end - start) / 1000)}
        visibleCount={spans.length}
        className="max-w-none"
      />
    </div>
  );
}
