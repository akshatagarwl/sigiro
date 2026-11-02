"use client";

import { createExplorer } from "@/components/sigiro/explorer";
import { TraceView } from "@/components/sigiro/trace-view";
import { STATUS_CHART } from "@/lib/sigiro/palette";
import { fromMicros, num } from "@/lib/sigiro/query";
import {
  col,
  createTableSchema,
  type InferTableType,
} from "@/lib/table-schema";

const STATUSES = ["error", "ok"] as const;
const STATUS_SQL = "CASE status_code WHEN 2 THEN 'error' ELSE 'ok' END";

function Waterfall(props: unknown) {
  const { trace_id: id } = props as { trace_id: string };
  return (
    <div className="w-full">
      <TraceView traceId={id} />
    </div>
  );
}

const schema = createTableSchema({
  status: col
    .enum(STATUSES)
    .label("Status")
    .display("level-indicator", {
      colorMap: { error: "var(--error)", ok: "var(--success)" },
    })
    .filterable("checkbox", {
      options: STATUSES.map((value) => ({ label: value, value })),
    })
    .defaultOpen()
    .hideHeader()
    .size(37)
    .sheet(),
  timestamp: col.presets
    .timestamp()
    .label("Time")
    .sortable(false)
    .defaultOpen()
    .commandDisabled()
    .size(200)
    .sheet(),
  // No fixed list: the checkbox options come from the facets the query returns.
  service: col
    .enum([] as readonly string[])
    .label("Service")
    .display("badge")
    .defaultOpen()
    .size(200)
    .sheet(),
  name: col
    .string()
    .label("Root span")
    .filterable("input")
    .minSize(280)
    .sheet(),
  duration: col.presets
    .latency("ms")
    .label("Duration")
    .sortable(false)
    .size(120)
    .sheet(),
  message: col
    .string()
    .label("Status message")
    .notFilterable()
    .hidden()
    .sheet(),
  trace_id: col
    .string()
    .label("Trace")
    .display("code")
    .filterable("input")
    .size(270)
    .sheet({
      label: "Waterfall",
      component: Waterfall,
      className: "flex-col items-start w-full gap-2",
    }),
});

type TraceRow = InferTableType<typeof schema.definition>;

export const TracesTable = createExplorer<TraceRow>({
  id: "traces",
  definition: schema.definition,
  chart: STATUS_CHART,
  source: {
    from: "sigiro_spans",
    base: "coalesce(parent_span_id, '') = ''",
    select: `timestamp, ${STATUS_SQL} AS status, service_name, span_name, round(duration / 1000, 1) AS duration_ms, status_message, trace_id`,
    series: { key: "status", sql: STATUS_SQL, values: STATUSES },
    facets: { service: "service_name" },
    sliders: { duration: "duration / 1000" },
    inputs: { name: "span_name", trace_id: "trace_id" },
    toRow: (row) => ({
      status: row.status === "error" ? "error" : "ok",
      timestamp: fromMicros(row.timestamp),
      service: String(row.service_name ?? ""),
      name: String(row.span_name ?? ""),
      duration: num(row.duration_ms),
      message: String(row.status_message ?? ""),
      trace_id: String(row.trace_id ?? ""),
    }),
  },
  rowId: (row) => row.trace_id,
  title: (row) => row.name,
});
