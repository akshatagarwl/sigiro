"use client";

import { DataTableAuto } from "@/components/data-table/data-table-auto";
import { Skeleton } from "@/components/ui/skeleton";
import { useSql, type Window } from "@/lib/sigiro/controls";
import {
  ERROR_RATE_SQL,
  FAILED_SQL,
  num,
  sqlString,
  ts,
} from "@/lib/sigiro/query";

const range = (w: Window) =>
  `timestamp >= ${ts(w.from)} AND timestamp < ${ts(w.to)}${w.service ? ` AND service_name = ${sqlString(w.service)}` : ""}`;

/** RED metrics per service, aggregated on the server, in the openstatus auto table. */
export default function ServicesPage() {
  const spans = useSql(
    "services",
    (w) =>
      `SELECT service_name AS service, count(*) AS spans, count(*) FILTER (WHERE ${FAILED_SQL}) AS failed_spans, ${ERROR_RATE_SQL} AS error_rate, round(quantile_cont(duration, 0.5) / 1000, 1) AS p50_ms, round(quantile_cont(duration, 0.95) / 1000, 1) AS p95_ms, round(quantile_cont(duration, 0.99) / 1000, 1) AS p99_ms, count(DISTINCT span_name) AS operations FROM sigiro_spans WHERE ${range(w)} GROUP BY 1`
  );
  const logs = useSql(
    "service-logs",
    (w) =>
      `SELECT service_name AS service, count(*) AS logs, count(*) FILTER (WHERE severity_number >= 17) AS error_logs FROM sigiro_logs WHERE ${range(w)} GROUP BY 1`
  );
  const error = spans.error ?? logs.error;
  if (error)
    return (
      <p className="text-destructive p-6 text-sm" role="alert">
        {error.message}
      </p>
    );
  if (!spans.data || !logs.data) return <Skeleton className="m-6 h-96" />;

  const byService = new Map<string, Record<string, string | number>>();
  for (const row of spans.data)
    byService.set(String(row.service), {
      service: String(row.service),
      spans: num(row.spans),
      failed_spans: num(row.failed_spans),
      // Percent, as on the Overview's Error rate card.
      error_rate: num(row.error_rate),
      p50_ms: num(row.p50_ms),
      p95_ms: num(row.p95_ms),
      p99_ms: num(row.p99_ms),
      operations: num(row.operations),
      logs: 0,
      error_logs: 0,
    });
  for (const row of logs.data) {
    const service = String(row.service);
    const entry = byService.get(service) ?? {
      service,
      spans: 0,
      failed_spans: 0,
      error_rate: 0,
      p50_ms: 0,
      p95_ms: 0,
      p99_ms: 0,
      operations: 0,
    };
    entry.logs = num(row.logs);
    entry.error_logs = num(row.error_logs);
    byService.set(service, entry);
  }
  const data = [...byService.values()].sort(
    (a, b) => Number(b.spans) - Number(a.spans)
  );

  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      data-table="services"
      data-rows={data.length}
    >
      <DataTableAuto data={data} />
    </div>
  );
}
