"use client";

import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import Link from "next/link";

import { createExplorer } from "@/components/sigiro/explorer";
import { LEVEL_CHART } from "@/lib/sigiro/palette";
import { fromMicros, type Level, LEVEL_SQL, LEVELS } from "@/lib/sigiro/query";
import {
  col,
  createTableSchema,
  type InferTableType,
} from "@/lib/table-schema";

/** Row tints from the data-table example's `getLevelRowClassName`. */
function levelRowClassName(level: string): string {
  switch (level) {
    case "warning":
      return "bg-warning/5 hover:bg-warning/10 data-[state=selected]:bg-warning/20 dark:bg-warning/10 dark:hover:bg-warning/20 dark:data-[state=selected]:bg-warning/30";
    case "error":
      return "bg-error/5 hover:bg-error/10 data-[state=selected]:bg-error/20 dark:bg-error/10 dark:hover:bg-error/20 dark:data-[state=selected]:bg-error/30";
    default:
      return "";
  }
}

function TraceLink(props: unknown) {
  const { trace_id: id } = props as { trace_id?: string };
  if (!id) return <span className="text-muted-foreground">none</span>;
  return (
    <Link className="font-mono underline" href={`/traces/?trace=${id}`}>
      {id}
    </Link>
  );
}

const schema = createTableSchema({
  level: col.presets
    .logLevel(LEVELS)
    .display("level-indicator")
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
  body: col.string().label("Message").filterable("input").minSize(400).sheet(),
  severity: col.string().label("Severity").notFilterable().hidden().sheet(),
  attributes: col
    .string()
    .label("Attributes")
    .display("code")
    .notFilterable()
    .hidden()
    .sheet(),
  trace_id: col
    .string()
    .label("Trace")
    .display("code")
    .filterable("input")
    .hidden()
    .sheet({ component: TraceLink }),
  uuid: col.presets.traceId().label("Row").hidden(),
});

type LogRow = InferTableType<typeof schema.definition>;

export const LogsTable = createExplorer<LogRow>({
  id: "logs",
  definition: schema.definition,
  chart: LEVEL_CHART,
  source: {
    from: "sigiro_logs",
    select: `timestamp, ${LEVEL_SQL} AS level, service_name, severity_text, body, CAST(log_attributes AS VARCHAR) AS attributes, trace_id, span_id`,
    series: { key: "level", sql: LEVEL_SQL, values: LEVELS },
    facets: { service: "service_name" },
    inputs: { body: "body", trace_id: "trace_id" },
    toRow: (row) => ({
      level: row.level as Level,
      timestamp: fromMicros(row.timestamp),
      service: String(row.service_name ?? ""),
      body: String(row.body ?? ""),
      severity: String(row.severity_text ?? ""),
      attributes: String(row.attributes ?? ""),
      trace_id: String(row.trace_id ?? ""),
      uuid: bytesToHex(
        sha256(
          new TextEncoder().encode(
            JSON.stringify([
              row.timestamp,
              row.service_name,
              row.body,
              row.span_id,
            ])
          )
        )
      ),
    }),
  },
  rowId: (row) => row.uuid,
  title: (row) => `${row.service} · ${row.severity || row.level}`,
  rowClassName: (row) => levelRowClassName(row.level),
});
