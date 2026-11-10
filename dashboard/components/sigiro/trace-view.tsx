"use client";

import { useQuery } from "@tanstack/react-query";

import { RequestState } from "@/components/observability/request-state";
import { TraceWorkspace } from "@/components/sigiro/trace-workspace";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { formatValue } from "@/lib/sigiro/profiles";
import { num, query, sqlString, type Row } from "@/lib/sigiro/query";

export function TraceView({ traceId }: { traceId: string }) {
  const valid = /^[0-9a-f]{32}$/i.test(traceId);
  const { data, error, isLoading, isFetching, refetch } = useQuery({
    queryKey: ["trace", traceId],
    enabled: valid,
    retry: false,
    queryFn: ({ signal }) =>
      query(
        `SELECT epoch_us(timestamp) AS start_us, duration, service_name, span_name, span_id, parent_span_id, status_code, status_message, span_attributes, resource_attributes, events_json, links_json FROM sigiro_spans WHERE trace_id = ${sqlString(traceId.toLowerCase())} ORDER BY timestamp, span_id LIMIT 2001`,
        signal
      ),
  });

  if (!valid)
    return <p className="text-destructive text-sm">Not a trace id.</p>;
  if (error)
    return (
      <div className="grid gap-3 rounded-lg border p-4">
        <p className="text-destructive text-sm" role="alert">
          {error.message}
        </p>
        <Button
          variant="outline"
          disabled={isFetching}
          onClick={() => void refetch()}
          className="w-fit"
        >
          Retry trace query
        </Button>
      </div>
    );
  if (isLoading || !data)
    return (
      <div className="grid gap-3">
        <RequestState kind="loading" title="Loading trace spans" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  if (data.length === 0)
    return (
      <p className="text-muted-foreground text-sm">
        No spans stored for this trace.
      </p>
    );

  const spans = data.slice(0, 2000);
  return (
    <div className="grid min-h-0 gap-4">
      <TraceWorkspace
        key={traceId}
        rows={spans}
        traceId={traceId.toLowerCase()}
        truncated={data.length > 2000}
      />
      <TraceProfiles traceId={traceId.toLowerCase()} spans={spans} />
    </div>
  );
}

/**
 * CPU samples linked to this trace's spans (OTLP Sample.link), one row per
 * span, each opening that span's flame graph on the Profiles page.
 */
function TraceProfiles({ traceId, spans }: { traceId: string; spans: Row[] }) {
  const { data, error, isFetching, refetch } = useQuery({
    queryKey: ["trace-profiles", traceId],
    retry: false,
    queryFn: ({ signal }) =>
      query(
        `SELECT span_id, any_value(service_name) AS service, sum(value) AS v, min(epoch_ms(timestamp)) AS t0, max(epoch_ms(timestamp)) AS t1 FROM sigiro_profiles WHERE trace_id = ${sqlString(traceId)} AND profile_type = 'cpu' GROUP BY span_id ORDER BY v DESC`,
        signal
      ),
  });
  if (error)
    return (
      <RequestState
        kind="error"
        title="Related profiles are unavailable"
        onRetry={() => refetch()}
        retrying={isFetching}
      >
        {error.message} The trace workspace remains available.
      </RequestState>
    );
  if (!data || data.length === 0) return null;
  const names = new Map(
    spans.map((row) => [String(row.span_id), String(row.span_name)])
  );
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
          <li
            key={String(row.span_id)}
            className="flex items-center gap-3 px-3 py-2"
          >
            <span className="min-w-0 flex-1 truncate">
              {names.get(String(row.span_id)) ?? "span"}{" "}
              <span className="text-muted-foreground font-mono text-xs">
                {String(row.span_id)}
              </span>
            </span>
            <span className="text-muted-foreground tabular-nums">
              {formatValue(num(row.v), "nanoseconds")}
            </span>
            <Button asChild variant="ghost" size="sm">
              <a
                href={link(String(row.service), String(row.span_id))}
                data-action="open-span-profile"
                data-span={String(row.span_id)}
              >
                Open profile
              </a>
            </Button>
          </li>
        ))}
      </ul>
    </div>
  );
}
