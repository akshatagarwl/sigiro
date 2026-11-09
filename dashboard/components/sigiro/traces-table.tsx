"use client";

import { createExplorer } from "@/components/sigiro/explorer";
import { STATUS_CHART } from "@/lib/sigiro/palette";
import { fromMicros, num } from "@/lib/sigiro/query";
import { field } from "@/lib/store/schema";
import { col, createTableSchema, type InferTableType } from "@/lib/table-schema";

const STATUSES = ["error", "ok"] as const;
const STATUS_SQL = "CASE status_code WHEN 2 THEN 'error' ELSE 'ok' END";

const schema = createTableSchema({
  name: col.string().label("Root span").filterable("input").minSize(320),
  service: col
    .enum([] as readonly string[])
    .label("Service")
    .display("badge")
    .defaultOpen()
    .size(180),
  status: col
    .enum(STATUSES)
    .label("Root status")
    .display("level-indicator", {
      colorMap: { error: "var(--error)", ok: "var(--success)" },
    })
    .filterable("checkbox", {
      options: STATUSES.map((value) => ({ label: value, value })),
    })
    .defaultOpen()
    .size(100),
  duration: col.presets.latency("ms").label("Duration").sortable(false).size(120),
  timestamp: col.presets.timestamp().label("Started").sortable(false).commandDisabled().size(200),
  message: col.string().label("Status message").notFilterable().hidden(),
  trace_id: col.string().label("Trace ID").display("code").filterable("input").hidden().size(270),
});

type TraceRow = InferTableType<typeof schema.definition>;

export const TracesTable = createExplorer<TraceRow>({
  id: "traces",
  detail: false,
  definition: schema.definition,
  filterOverrides: { service: field.array(field.string()) },
  chart: STATUS_CHART,
  source: {
    from: "sigiro_spans",
    base: "coalesce(parent_span_id, '') IN ('', '0000000000000000')",
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
