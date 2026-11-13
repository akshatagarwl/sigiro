"use client";

import { EChartsLatencyAreaChart } from "@/components/evilcharts/blocks/latency-echarts-area-chart";
import { EvilBarChart } from "@/components/evilcharts/charts/recharts-bar-chart";
import type { ChartConfig } from "@/components/evilcharts/ui/recharts-chart";
import { RequestState } from "@/components/observability/request-state";
import { SectionCards } from "@/components/section-cards";
import { SetupEmpty } from "@/components/sigiro/setup-empty";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  RANGES,
  useControls,
  useSql,
  type Window,
} from "@/lib/sigiro/controls";
import { LEVEL_CHART, STATUS_CHART } from "@/lib/sigiro/palette";
import {
  bucketFor,
  ERROR_RATE_SQL,
  FAILED_SQL,
  LEVEL_SQL,
  num,
  type Row,
  sqlString,
  ts,
} from "@/lib/sigiro/query";

const range = (w: Window) =>
  `timestamp >= ${ts(w.from)} AND timestamp < ${ts(w.to)}${w.service ? ` AND service_name = ${sqlString(w.service)}` : ""}`;

const previous = (w: Window): Window => {
  const span = w.to.getTime() - w.from.getTime();
  return { ...w, from: new Date(w.from.getTime() - span), to: w.from };
};

const bucketed = (w: Window) =>
  `epoch_ms(time_bucket(INTERVAL '${bucketFor(w.from, w.to).sql}', timestamp))`;

const formatCount = (value: number) =>
  Intl.NumberFormat(undefined, { notation: "compact" }).format(value);
const formatMs = (value: number | null) =>
  value === null
    ? "–"
    : value >= 1000
      ? `${(value / 1000).toFixed(2)} s`
      : `${value.toFixed(1)} ms`;
const change = (now: number, before: number) =>
  before > 0 ? ((now - before) / before) * 100 : undefined;

/** Every bucket in the window, zero-filled, labelled for the x axis. */
function axis<K extends string>(
  rows: Row[] | undefined,
  w: { from: Date; to: Date },
  keys: readonly K[],
  empty: number | null = 0
) {
  const { ms } = bucketFor(w.from, w.to);
  const byTime = new Map((rows ?? []).map((row) => [num(row.t), row]));
  const points: ({ time: string } & Record<K, number | null>)[] = [];
  for (
    let t = Math.floor(w.from.getTime() / ms) * ms;
    t < w.to.getTime();
    t += ms
  ) {
    const row = byTime.get(t);
    const point = {
      time: new Date(t).toLocaleString(undefined, {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      }),
    } as { time: string } & Record<K, number | null>;
    for (const key of keys)
      (point as Record<string, number | null>)[key] =
        row && row[key] !== null ? num(row[key]) : empty;
    points.push(point);
  }
  return points;
}

function ChartCard({
  title,
  description,
  children,
  error,
}: {
  title: string;
  description: string;
  children: React.ReactNode;
  error?: Error | null;
}) {
  return (
    <Card className="@container/card">
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="h-72 px-2 sm:px-6">
        {error ? (
          <p className="text-muted-foreground text-sm" data-panel-error>
            {title} unavailable. Retry from the error notice above.
          </p>
        ) : (
          children
        )}
      </CardContent>
    </Card>
  );
}

function StackedBars({
  data,
  config,
  isLoading,
  name,
}: {
  data: Record<string, unknown>[];
  config: ChartConfig;
  isLoading: boolean;
  name: string;
}) {
  return (
    <div className="h-full w-full" data-chart-name={name}>
      <EvilBarChart
        data={data}
        config={config}
        stackType="stacked"
        className="h-full w-full"
        isLoading={isLoading}
      >
        <EvilBarChart.Grid />
        <EvilBarChart.XAxis dataKey="time" />
        <EvilBarChart.Legend isClickable />
        <EvilBarChart.Tooltip />
        {Object.keys(config).map((key) => (
          <EvilBarChart.Bar key={key} dataKey={key} isClickable />
        ))}
      </EvilBarChart>
    </div>
  );
}

export default function OverviewPage() {
  const controls = useControls();
  const win = { from: controls.from, to: controls.to };

  const kpis = useSql("kpis", (w) => {
    const p = previous(w);
    return `SELECT
      (SELECT count(*) FROM sigiro_spans WHERE ${range(w)}) AS spans,
      (SELECT count(*) FROM sigiro_spans WHERE ${range(p)}) AS spans_before,
      (SELECT count(*) FROM sigiro_spans WHERE ${range(w)} AND ${FAILED_SQL}) AS errors,
      (SELECT count(*) FROM sigiro_spans WHERE ${range(p)} AND ${FAILED_SQL}) AS errors_before,
      (SELECT ${ERROR_RATE_SQL} FROM sigiro_spans WHERE ${range(w)}) AS error_rate,
      (SELECT quantile_cont(duration, 0.95) / 1000 FROM sigiro_spans WHERE ${range(w)}) AS p95_ms,
      (SELECT count(*) FROM sigiro_logs WHERE ${range(w)}) AS logs,
      (SELECT count(*) FROM sigiro_logs WHERE ${range(w)} AND severity_number >= 17) AS error_logs,
      (SELECT count(*) FROM sigiro_logs WHERE ${range(p)} AND severity_number >= 17) AS error_logs_before`;
  });
  const requests = useSql(
    "requests",
    (w) =>
      `SELECT ${bucketed(w)} AS t, count(*) FILTER (WHERE NOT ${FAILED_SQL}) AS ok, count(*) FILTER (WHERE ${FAILED_SQL}) AS error FROM sigiro_spans WHERE ${range(w)} GROUP BY 1`
  );
  const latency = useSql(
    "latency",
    (w) =>
      `SELECT ${bucketed(w)} AS t, round(quantile_cont(duration, 0.5) / 1000, 1) AS p50, round(quantile_cont(duration, 0.95) / 1000, 1) AS p95, round(quantile_cont(duration, 0.99) / 1000, 1) AS p99 FROM sigiro_spans WHERE ${range(w)} GROUP BY 1`
  );
  const logs = useSql(
    "log-volume",
    (w) =>
      `SELECT ${bucketed(w)} AS t, ${["error", "warning", "info", "debug"].map((level) => `count(*) FILTER (WHERE ${LEVEL_SQL} = '${level}') AS ${level}`).join(", ")} FROM sigiro_logs WHERE ${range(w)} GROUP BY 1`
  );
  const overall = useSql(
    "latency-overall",
    (w) =>
      `SELECT round(quantile_cont(duration, 0.5) / 1000, 1) AS p50, round(quantile_cont(duration, 0.95) / 1000, 1) AS p95, round(quantile_cont(duration, 0.99) / 1000, 1) AS p99 FROM sigiro_spans WHERE ${range(w)}`
  );

  const k = kpis.data?.[0];
  const spans = num(k?.spans);
  const errors = num(k?.errors);
  const errorLogs = num(k?.error_logs);
  const panels = [kpis, requests, latency, logs, overall];
  const error = panels.find((q) => q.error)?.error;
  const loading = panels.some((q) => q.isLoading);
  const updating = !loading && panels.some((q) => q.isFetching);
  const ready = kpis.data !== undefined;
  const missingValue = kpis.isLoading ? "…" : "—";
  const empty =
    !loading &&
    !updating &&
    panels.every((q) => q.isSuccess) &&
    spans === 0 &&
    num(k?.logs) === 0;
  const o = overall.data?.[0];

  return (
    <div
      className="flex flex-col gap-4 py-4 md:gap-6 md:py-6"
      data-page="overview"
      data-state={
        error
          ? "error"
          : loading
            ? "loading"
            : updating
              ? "updating"
              : empty
                ? "empty"
                : "ready"
      }
    >
      {error && (
        <div className="px-4 lg:px-6">
          <RequestState
            kind="error"
            title="Some overview panels are unavailable"
            onRetry={controls.refresh}
            retrying={panels.some((q) => q.isFetching)}
          >
            {error.message} Available values are the last loaded snapshots, not
            zero values for failed panels.
          </RequestState>
        </div>
      )}
      {!error && (loading || updating) && (
        <div className="px-4 lg:px-6">
          <RequestState
            kind={loading ? "loading" : "updating"}
            title={loading ? "Loading overview" : "Updating overview"}
          >
            {updating &&
              "Showing the last loaded snapshots until refresh completes."}
          </RequestState>
        </div>
      )}
      <SectionCards
        items={[
          {
            label: "Spans",
            value: ready ? formatCount(spans) : missingValue,
            change: change(spans, num(k?.spans_before)),
            headline: "Requests and internal spans",
            note: "vs the previous window",
          },
          {
            label: "Failed spans",
            value: ready ? formatCount(errors) : missingValue,
            change: change(errors, num(k?.errors_before)),
            riseIsBad: true,
            headline: "Spans that ended in an error",
            note: "vs the previous window",
          },
          {
            label: "Error rate",
            value: ready
              ? k?.error_rate === null || k?.error_rate === undefined
                ? "–"
                : `${num(k.error_rate).toFixed(2)}%`
              : missingValue,
            headline: "Failed ÷ all spans",
            note: "percentage of spans with ERROR status",
          },
          {
            label: "p95 latency",
            value: ready
              ? formatMs(k?.p95_ms === null ? null : num(k?.p95_ms))
              : missingValue,
            headline: "Across all spans",
            note: "95% of spans finished faster",
          },
          {
            label: "Error logs",
            value: ready ? formatCount(errorLogs) : missingValue,
            change: change(errorLogs, num(k?.error_logs_before)),
            riseIsBad: true,
            headline: `of ${formatCount(num(k?.logs))} log lines`,
            note: "logged at error level or worse",
          },
        ]}
      />
      {empty && (
        <div className="px-4 lg:px-6">
          {controls.service ? (
            <RequestState
              kind="empty"
              title="No telemetry matches the selected service and window"
            >
              Choose another service or time range above. Other services may
              have data.
            </RequestState>
          ) : (
            <SetupEmpty range={RANGES[controls.range].label.toLowerCase()} />
          )}
        </div>
      )}
      <div className="grid gap-4 px-4 lg:grid-cols-2 lg:px-6">
        <ChartCard
          title="Requests"
          error={requests.error}
          description="Spans over time, succeeded and failed"
        >
          <StackedBars
            name="requests"
            data={axis(requests.data, win, ["ok", "error"] as const)}
            config={STATUS_CHART}
            isLoading={requests.isLoading}
          />
        </ChartCard>
        <ChartCard
          title="Latency"
          error={latency.error}
          description="How long spans took: median, 95th and 99th percentile"
        >
          <div className="h-full w-full" data-chart-name="latency">
            <EChartsLatencyAreaChart
              data={axis(
                latency.data,
                win,
                ["p50", "p95", "p99"] as const,
                null
              )}
              latest={{
                p50:
                  o?.p50 === null || o?.p50 === undefined ? null : num(o.p50),
                p95:
                  o?.p95 === null || o?.p95 === undefined ? null : num(o.p95),
                p99:
                  o?.p99 === null || o?.p99 === undefined ? null : num(o.p99),
              }}
              isLoading={latency.isLoading}
            />
          </div>
        </ChartCard>
        <div className="lg:col-span-2">
          <ChartCard
            title="Log volume"
            error={logs.error}
            description="Log lines over time, by level"
          >
            <StackedBars
              name="logs"
              data={axis(logs.data, win, [
                "error",
                "warning",
                "info",
                "debug",
              ] as const)}
              config={LEVEL_CHART}
              isLoading={logs.isLoading}
            />
          </ChartCard>
        </div>
      </div>
    </div>
  );
}
