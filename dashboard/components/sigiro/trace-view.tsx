"use client";

import { useQuery } from "@tanstack/react-query";

import {
  TraceWaterfall,
  type TraceSpan,
} from "@/components/assistant-ui/elements/trace-waterfall";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { formatValue } from "@/lib/sigiro/profiles";
import { num, query, sqlString, type Row } from "@/lib/sigiro/query";

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
    <div className="grid gap-4">
      <div data-waterfall={spans.length}>
        <TraceWaterfall
          spans={spans}
          totalMs={Math.round((end - start) / 1000)}
          visibleCount={spans.length}
          className="max-w-none"
        />
      </div>
      <TraceProfiles traceId={traceId.toLowerCase()} spans={data} />
    </div>
  );
}

/**
 * CPU samples linked to this trace's spans (OTLP Sample.link), one row per
 * span, each opening that span's flame graph on the Profiles page.
 */
function TraceProfiles({ traceId, spans }: { traceId: string; spans: Row[] }) {
  const { data } = useQuery({
    queryKey: ["trace-profiles", traceId],
    queryFn: ({ signal }) =>
      query(
        `SELECT span_id, any_value(service_name) AS service, sum(value) AS v, min(epoch_ms(timestamp)) AS t0, max(epoch_ms(timestamp)) AS t1 FROM sigiro_profiles WHERE trace_id = ${sqlString(traceId)} AND profile_type = 'cpu' GROUP BY span_id ORDER BY v DESC`,
        signal
      ),
  });
  if (!data || data.length === 0) return null;
  const names = new Map(spans.map((row) => [String(row.span_id), String(row.span_name)]));
  const t0 = Math.min(...data.map((row) => num(row.t0))) - 60_000;
  const t1 = Math.max(...data.map((row) => num(row.t1))) + 60_000;
  const link = (service: string, span?: string) =>
    `/dashboard/profiles/?service=${encodeURIComponent(service)}&ptype=cpu&trace=${traceId}${span ? `&span=${span}` : ""}&from=${t0}&to=${t1}`;
  const service = String(data[0].service);
  return (
    <div className="grid gap-2" data-trace-profiles={data.length}>
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">CPU profile</h3>
        <Button asChild variant="outline" size="sm">
          <a href={link(service)} data-action="open-trace-profile">
            Whole trace
          </a>
        </Button>
      </div>
      <ul className="divide-y rounded-md border text-sm">
        {data.map((row) => (
          <li key={String(row.span_id)} className="flex items-center gap-3 px-3 py-2">
            <span className="min-w-0 flex-1 truncate">
              {names.get(String(row.span_id)) ?? "span"}{" "}
              <span className="text-muted-foreground font-mono text-xs">{String(row.span_id)}</span>
            </span>
            <span className="text-muted-foreground tabular-nums">{formatValue(num(row.v), "nanoseconds")}</span>
            <Button asChild variant="ghost" size="sm">
              <a href={link(String(row.service), String(row.span_id))} data-action="open-span-profile" data-span={String(row.span_id)}>
                Open profile
              </a>
            </Button>
          </li>
        ))}
      </ul>
    </div>
  );
}
