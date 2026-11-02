import * as Option from "effect/Option";
import * as Schema from "effect/Schema";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Line,
  LineChart,
  XAxis,
  YAxis,
} from "recharts";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  type ChartConfig,
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { BROWSER_CLIENT_SCOPE } from "@/lib/browser-client";
import { createBrowserOAuth } from "@/lib/browser-oauth";

import {
  type Cell,
  type LoadReport,
  QueryError,
  type Result,
  SERVER_TABLES,
  type TraceData,
  WINDOWS,
  WINDOW_KEYS,
  type Window,
  type WindowKey,
  fetchTrace,
  loadWindow,
  runLocal,
  sqlString,
  windowFor,
} from "./telemetry";

/**
 * `/dashboard/`: services, traces, logs and SQL over one tenant's telemetry.
 *
 * Everything on screen is a stock shadcn/ui component (src/components/ui) fed
 * by a SQL string run in DuckDB-WASM; the only hand-drawn view is the trace
 * waterfall, which no component library ships. To add a panel, add a query
 * below and hand its `Result` to `Grid` or `TimeChart`.
 */

const auth = createBrowserOAuth({
  redirectPath: "/dashboard/",
  scope: BROWSER_CLIENT_SCOPE,
  transientKey: "sigiro.dashboard.oauth",
  tokenKey: "sigiro.dashboard.token",
});

type Phase =
  | { kind: "starting" }
  | { kind: "signed-out"; reason?: string }
  | { kind: "loading"; reports: LoadReport[] }
  | { kind: "ready"; reports: LoadReport[] }
  | { kind: "error"; message: string };

const ALL = "__all__";
const SEVERITIES = {
  "0": "All levels",
  "13": "Warn and above",
  "17": "Error and above",
} as const;

const DEFAULT_SQL =
  "SELECT service_name, severity_text, count(*) AS n\nFROM sigiro_logs\nGROUP BY ALL\nORDER BY n DESC";

const AccessTokenClaims = Schema.fromJsonString(
  Schema.Struct({ tid: Schema.String })
);

/** The tenant the query API will scope to: the `tid` claim. Display only; the server verifies the token. */
const tenantOf = (token: string): string | undefined => {
  try {
    const payload = atob(
      (token.split(".")[1] ?? "").replaceAll("-", "+").replaceAll("_", "/")
    );
    return Option.getOrUndefined(
      Schema.decodeUnknownOption(AccessTokenClaims)(payload)
    )?.tid;
  } catch {
    return undefined;
  }
};

const formatUtc = (date: Date) =>
  `${date.toISOString().slice(0, 16).replace("T", " ")} UTC`;

const where = (clauses: (string | false | undefined)[]) => {
  const kept = clauses.filter(Boolean);
  return kept.length > 0 ? `WHERE ${kept.join(" AND ")}` : "";
};

const ilike = (column: string, search: string) =>
  `${column} ILIKE ${sqlString(`%${search}%`)}`;

/** Every query the page runs locally, for one window and one set of filters. */
const queriesFor = (
  w: Window,
  service: string,
  severity: number,
  search: string
) => {
  const svc = service !== ALL && `service_name = ${sqlString(service)}`;
  const text = search.trim();
  const axis = `SELECT b AS t FROM range(TIMESTAMP '${w.from.toISOString().slice(0, 19)}', TIMESTAMP '${w.to.toISOString().slice(0, 19)}', INTERVAL '${WINDOWS[w.key].bucket}') r(b)`;
  const bucket = `time_bucket(INTERVAL '${WINDOWS[w.key].bucket}', timestamp)`;
  const red = `SELECT service_name, span_name, sum(n) AS n, sum(n) FILTER (WHERE status_code = 2) AS errors, sum(total_us) AS total_us FROM span_buckets ${where([svc])} GROUP BY 1, 2`;
  const sample = `SELECT service_name, span_name, quantile_cont(duration, 0.5) / 1000 AS p50, quantile_cont(duration, 0.95) / 1000 AS p95 FROM sigiro_spans ${where([svc])} GROUP BY 1, 2`;
  return {
    kpis: `SELECT (SELECT coalesce(sum(n), 0) FROM span_buckets ${where([svc])}) AS spans, (SELECT coalesce(sum(n), 0) FROM span_buckets ${where([svc, "status_code = 2"])}) AS errors, (SELECT round(quantile_cont(duration, 0.95) / 1000, 1) FROM sigiro_spans ${where([svc])}) AS p95_ms, (SELECT coalesce(sum(n), 0) FROM log_buckets ${where([svc])}) AS logs, (SELECT coalesce(sum(n), 0) FROM log_buckets ${where([svc, "severity_number >= 17"])}) AS error_logs`,
    requests: `SELECT strftime(a.t, '%m-%d %H:%M') AS t, coalesce(sum(s.n) FILTER (WHERE s.status_code <> 2), 0) AS ok, coalesce(sum(s.n) FILTER (WHERE s.status_code = 2), 0) AS errors FROM (${axis}) a LEFT JOIN (SELECT * FROM span_buckets ${where([svc])}) s ON s.bucket = a.t GROUP BY a.t ORDER BY a.t`,
    latency: `SELECT strftime(a.t, '%m-%d %H:%M') AS t, round(quantile_cont(s.duration, 0.5) / 1000, 1) AS p50, round(quantile_cont(s.duration, 0.95) / 1000, 1) AS p95, round(quantile_cont(s.duration, 0.99) / 1000, 1) AS p99 FROM (${axis}) a LEFT JOIN (SELECT ${bucket} AS b, duration FROM sigiro_spans ${where([svc])}) s ON s.b = a.t GROUP BY a.t ORDER BY a.t`,
    logVolume: `SELECT strftime(a.t, '%m-%d %H:%M') AS t, coalesce(sum(l.n) FILTER (WHERE l.severity_number < 13), 0) AS info, coalesce(sum(l.n) FILTER (WHERE l.severity_number BETWEEN 13 AND 16), 0) AS warn, coalesce(sum(l.n) FILTER (WHERE l.severity_number >= 17), 0) AS error FROM (${axis}) a LEFT JOIN (SELECT * FROM log_buckets ${where([svc])}) l ON l.bucket = a.t GROUP BY a.t ORDER BY a.t`,
    services: `SELECT r.service_name AS service, sum(r.n) AS requests, sum(r.errors) AS errors, round(100.0 * coalesce(sum(r.errors), 0) / sum(r.n), 2) AS error_pct, round(sum(r.total_us) / sum(r.n) / 1000, 1) AS avg_ms, round(max(p.p95), 1) AS p95_ms FROM (${red}) r LEFT JOIN (${sample}) p USING (service_name, span_name) GROUP BY 1 ORDER BY requests DESC`,
    operations: `SELECT r.service_name AS service, r.span_name AS operation, r.n AS requests, coalesce(r.errors, 0) AS errors, round(100.0 * coalesce(r.errors, 0) / r.n, 2) AS error_pct, round(p.p50, 1) AS p50_ms, round(p.p95, 1) AS p95_ms FROM (${red}) r LEFT JOIN (${sample}) p USING (service_name, span_name) ${where([text && ilike("r.span_name", text)])} ORDER BY requests DESC LIMIT 100`,
    traces: `SELECT timestamp AS time, service_name AS service, span_name AS root, round(duration / 1000, 1) AS duration_ms, CASE status_code WHEN 2 THEN 'error' ELSE 'ok' END AS status, trace_id FROM sigiro_spans ${where([svc, "coalesce(parent_span_id, '') = ''", text && `(${ilike("span_name", text)} OR trace_id = ${sqlString(text.toLowerCase())})`])} ORDER BY timestamp DESC LIMIT 200`,
    failed: `SELECT service_name AS service, span_name AS operation, status_message AS message, sum(n) AS count, any_value(trace_id) AS trace_id FROM failed_spans ${where([svc, text && `(${ilike("span_name", text)} OR ${ilike("coalesce(status_message, '')", text)})`])} GROUP BY 1, 2, 3 ORDER BY count DESC LIMIT 100`,
    logs: `SELECT timestamp AS time, service_name AS service, severity_text AS severity, body, trace_id FROM sigiro_logs ${where([svc, severity > 0 && `severity_number >= ${severity}`, text && ilike("body", text)])} ORDER BY timestamp DESC LIMIT 200`,
    patterns: `SELECT coalesce(any_value(template_text), any_value(example)) AS pattern, service_name AS service, CASE WHEN max(severity_number) >= 17 THEN 'ERROR' ELSE 'WARN' END AS severity, sum(n) AS count FROM log_templates ${where([svc, severity > 13 && `severity_number >= ${severity}`, text && `(${ilike("template_text", text)} OR ${ilike("example", text)})`])} GROUP BY template_id, service_name ORDER BY count DESC LIMIT 100`,
  };
};
type Panels = Partial<Record<keyof ReturnType<typeof queriesFor>, Result>>;

export function Dashboard() {
  const [phase, setPhase] = useState<Phase>({ kind: "starting" });
  const [windowKey, setWindowKey] = useState<WindowKey>("24h");
  const [loaded, setLoaded] = useState<Window | null>(null);
  const [tenant, setTenant] = useState<string>();
  const [service, setService] = useState(ALL);
  const [severity, setSeverity] = useState("0");
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState("overview");
  const [traceId, setTraceId] = useState<string>();
  const [services, setServices] = useState<string[]>([]);
  const [panels, setPanels] = useState<Panels>({});
  const [panelError, setPanelError] = useState<string>();
  const loads = useRef(0);

  const load = useCallback(async (key: WindowKey) => {
    const token = auth.currentToken();
    if (!token) {
      setPhase({
        kind: "signed-out",
        reason: "Your session ended. Sign in again to load telemetry.",
      });
      return;
    }
    const id = ++loads.current;
    setTenant(tenantOf(token));
    const w = windowFor(key);
    setPhase({ kind: "loading", reports: [] });
    try {
      const reports = await loadWindow(token, w, (report) => {
        if (id === loads.current)
          setPhase((current) =>
            current.kind === "loading"
              ? { kind: "loading", reports: [...current.reports, report] }
              : current
          );
      });
      if (id !== loads.current) return;
      const list = await runLocal(
        "SELECT DISTINCT service_name FROM (SELECT service_name FROM log_buckets UNION ALL SELECT service_name FROM span_buckets) WHERE service_name IS NOT NULL ORDER BY 1"
      );
      setServices(list.rows.map((row) => String(row[0])));
      setLoaded(w);
      setPhase({ kind: "ready", reports });
    } catch (error) {
      if (id !== loads.current) return;
      if (error instanceof QueryError && error.status === 401) {
        auth.forget();
        setPhase({
          kind: "signed-out",
          reason: "Sigiro rejected the access token. Sign in again.",
        });
        return;
      }
      setPhase({
        kind: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  useEffect(() => {
    const start = async () => {
      try {
        await auth.handleCallback();
      } catch (error) {
        setPhase({
          kind: "signed-out",
          reason: `Sign-in did not complete: ${error instanceof Error ? error.message : String(error)}`,
        });
        return;
      }
      if (auth.currentToken()) await load("24h");
      else setPhase({ kind: "signed-out" });
    };
    void start();
  }, [load]);

  const queries = useMemo(
    () =>
      loaded
        ? queriesFor(loaded, service, Number(severity), search)
        : undefined,
    [loaded, service, severity, search]
  );

  useEffect(() => {
    if (!queries || phase.kind !== "ready") return;
    let live = true;
    const refresh = async () => {
      try {
        const entries = await Promise.all(
          Object.entries(queries).map(
            async ([name, sql]) => [name, await runLocal(sql)] as const
          )
        );
        if (!live) return;
        setPanels(Object.fromEntries(entries));
        setPanelError(undefined);
      } catch (error) {
        if (live)
          setPanelError(error instanceof Error ? error.message : String(error));
      }
    };
    void refresh();
    return () => {
      live = false;
    };
  }, [queries, phase.kind]);

  if (phase.kind === "starting")
    return <Shell state="starting">Checking sign-in…</Shell>;

  if (phase.kind === "signed-out")
    return (
      <Shell>
        <Card className="mx-auto max-w-md" data-state="signed-out">
          <CardHeader>
            <CardTitle>Sign in to read your telemetry</CardTitle>
            <CardDescription>
              The dashboard reads your workspace through the query API with a
              read-only token. The token stays in this tab and expires after 15
              minutes.
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-4">
            {phase.reason && (
              <p className="text-destructive text-sm" role="alert">
                {phase.reason}
              </p>
            )}
            <div>
              <Button
                data-action="sign-in"
                onClick={() =>
                  void auth.beginAuthorization().catch(() => undefined)
                }
              >
                Sign in
              </Button>
            </div>
          </CardContent>
        </Card>
      </Shell>
    );

  if (phase.kind === "error")
    return (
      <Shell>
        <Card data-state="error" role="alert">
          <CardHeader>
            <CardTitle>The dashboard could not load</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <pre className="bg-muted overflow-x-auto rounded-md p-3 font-mono text-xs whitespace-pre-wrap">
              {phase.message}
            </pre>
            <div>
              <Button variant="outline" onClick={() => void load(windowKey)}>
                Try again
              </Button>
            </div>
          </CardContent>
        </Card>
      </Shell>
    );

  const reports = phase.reports;
  const loading = phase.kind === "loading";
  const truncated = reports.filter((report) => report.truncated);
  const empty =
    !loading &&
    reports
      .filter((report) =>
        ["log_buckets", "span_buckets"].includes(report.table)
      )
      .every((report) => report.rows === 0);
  const kpi = panels.kpis?.rows[0] ?? [];
  const openTrace = (id: string) => setTraceId(id);

  return (
    <Shell>
      <div className="grid gap-4" data-state={phase.kind}>
        <header className="flex flex-wrap items-center gap-2">
          <Select value={service} onValueChange={setService} disabled={loading}>
            <SelectTrigger aria-label="Service" className="w-56">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All services</SelectItem>
              {services.map((name) => (
                <SelectItem key={name} value={name}>
                  {name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Input
            type="search"
            aria-label="Search"
            placeholder="Search operations, messages, trace id"
            className="max-w-sm min-w-[14rem] flex-1"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <div className="ml-auto flex items-center gap-2">
            <Select
              value={windowKey}
              disabled={loading}
              onValueChange={(key) => {
                const picked = WINDOW_KEYS.find((known) => known === key);
                if (!picked) return;
                setWindowKey(picked);
                void load(picked);
              }}
            >
              <SelectTrigger aria-label="Time window" className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {WINDOW_KEYS.map((key) => (
                  <SelectItem key={key} value={key}>
                    Last {key}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              variant="outline"
              disabled={loading}
              onClick={() => void load(windowKey)}
            >
              Refresh
            </Button>
          </div>
        </header>

        <p
          className="text-muted-foreground text-xs"
          data-window
          data-progress={reports.length}
          role="status"
        >
          {loading
            ? `Loading ${reports.length + 1} of ${SERVER_TABLES.length}: ${SERVER_TABLES[reports.length]?.name ?? ""}`
            : loaded
              ? `${formatUtc(loaded.from)} → ${formatUtc(loaded.to)} · ${reports.length} tables in DuckDB-WASM`
              : ""}
          {tenant ? ` · workspace ${tenant}` : ""}
          {truncated.length > 0 &&
            ` · capped at the row limit: ${truncated.map((report) => report.table).join(", ")}`}
        </p>

        {loading && (
          <div className="grid gap-4 md:grid-cols-5">
            {Array.from({ length: 5 }, (_, index) => (
              <Skeleton key={index} className="h-24" />
            ))}
            <Skeleton className="h-72 md:col-span-5" />
          </div>
        )}

        {empty && (
          <Card data-state="empty">
            <CardHeader>
              <CardTitle>No telemetry in this window</CardTitle>
              <CardDescription>
                Workspace{" "}
                <code className="font-mono">{tenant ?? "unknown"}</code> has no
                logs or spans between {loaded ? formatUtc(loaded.from) : "—"}{" "}
                and {loaded ? formatUtc(loaded.to) : "—"}. Pick a longer window,
                or check that your services export to this workspace.
              </CardDescription>
            </CardHeader>
          </Card>
        )}

        {panelError && (
          <p className="text-destructive text-sm" role="alert">
            {panelError}
          </p>
        )}

        {!loading && (
          <Tabs value={tab} onValueChange={setTab}>
            <TabsList>
              <TabsTrigger value="overview">Overview</TabsTrigger>
              <TabsTrigger value="services">Services</TabsTrigger>
              <TabsTrigger value="traces">Traces</TabsTrigger>
              <TabsTrigger value="logs">Logs</TabsTrigger>
              <TabsTrigger value="sql">SQL</TabsTrigger>
            </TabsList>

            <TabsContent value="overview" className="grid gap-4">
              <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
                <Stat label="Spans" value={num(kpi[0])} />
                <Stat label="Failed spans" value={num(kpi[1])} tone="error" />
                <Stat
                  label="Error rate"
                  value={pct(kpi[1], kpi[0])}
                  tone="error"
                />
                <Stat
                  label="p95 latency"
                  value={
                    kpi[2] === null || kpi[2] === undefined
                      ? undefined
                      : Number(kpi[2]) >= 1000
                        ? `${(Number(kpi[2]) / 1000).toFixed(1)} s`
                        : `${kpi[2]} ms`
                  }
                />
                <Stat
                  label="Error logs"
                  value={num(kpi[4])}
                  note={
                    kpi[3] === undefined ? undefined : `of ${num(kpi[3])} logs`
                  }
                />
              </div>
              <div className="grid gap-4 lg:grid-cols-2">
                <TimeChart
                  title="Requests"
                  note={`spans per ${WINDOWS[windowKey].bucket}, exact`}
                  result={panels.requests}
                  config={{
                    ok: { label: "OK", color: "var(--chart-1)" },
                    errors: { label: "Errors", color: "var(--chart-2)" },
                  }}
                  stacked
                />
                <TimeChart
                  title="Latency"
                  note="ms, from the span sample"
                  result={panels.latency}
                  config={{
                    p50: { label: "p50", color: "var(--chart-3)" },
                    p95: { label: "p95", color: "var(--chart-1)" },
                    p99: { label: "p99", color: "var(--chart-2)" },
                  }}
                />
              </div>
              <Section
                title="Services"
                note="click a row to filter every view to it"
              >
                <Grid
                  name="services"
                  result={panels.services}
                  onRow={(row) => setService(String(row[0]))}
                />
              </Section>
            </TabsContent>

            <TabsContent value="services" className="grid gap-4">
              <Section
                title="Services"
                note="requests and errors exact, latency from the span sample"
              >
                <Grid
                  name="services"
                  result={panels.services}
                  onRow={(row) => setService(String(row[0]))}
                />
              </Section>
              <Section title="Operations" note="per span name">
                <Grid name="operations" result={panels.operations} />
              </Section>
            </TabsContent>

            <TabsContent value="traces" className="grid gap-4">
              <Section
                title="Traces"
                note="root spans from the span sample, newest first · click to open"
              >
                <Grid
                  name="traces"
                  result={panels.traces}
                  onTrace={openTrace}
                />
              </Section>
              <Section
                title="Errors"
                note="failed spans grouped by operation and message, exact"
              >
                <Grid
                  name="failed"
                  result={panels.failed}
                  onTrace={openTrace}
                />
              </Section>
            </TabsContent>

            <TabsContent value="logs" className="grid gap-4">
              <div className="flex items-center gap-2">
                <Select value={severity} onValueChange={setSeverity}>
                  <SelectTrigger aria-label="Minimum severity" className="w-48">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {Object.entries(SEVERITIES).map(([value, label]) => (
                      <SelectItem key={value} value={value}>
                        {label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <TimeChart
                title="Log volume"
                note={`per ${WINDOWS[windowKey].bucket}, exact`}
                result={panels.logVolume}
                config={{
                  info: { label: "Info", color: "var(--chart-5)" },
                  warn: { label: "Warn", color: "var(--chart-4)" },
                  error: { label: "Error", color: "var(--chart-2)" },
                }}
                stacked
              />
              <Section
                title="Patterns"
                note="warning-and-above log templates, exact counts"
              >
                <Grid name="templates" result={panels.patterns} />
              </Section>
              <Section title="Logs" note="newest 200 of the loaded rows">
                <Grid name="logs" result={panels.logs} onTrace={openTrace} />
              </Section>
            </TabsContent>

            <TabsContent value="sql">
              <SqlBox reports={reports} />
            </TabsContent>
          </Tabs>
        )}
      </div>
      <TraceSheet traceId={traceId} onClose={() => setTraceId(undefined)} />
    </Shell>
  );
}

const num = (value: Cell | undefined) =>
  value === null || value === undefined
    ? "—"
    : Number(value).toLocaleString("en");
const pct = (part: Cell | undefined, whole: Cell | undefined) =>
  Number(whole) > 0
    ? `${((100 * Number(part)) / Number(whole)).toFixed(2)}%`
    : undefined;

function Shell({
  children,
  state,
}: {
  children: React.ReactNode;
  state?: string;
}) {
  return (
    <div className="w-full px-4 py-8 text-sm sm:px-6" data-state={state}>
      {children}
    </div>
  );
}

function Stat({
  label,
  value,
  note,
  tone,
}: {
  label: string;
  value: string | undefined;
  note?: string;
  tone?: "error";
}) {
  const shown = value ?? "—";
  const bad =
    tone === "error" && shown !== "—" && shown !== "0" && shown !== "0.00%";
  return (
    <Card className="gap-1 py-4">
      <CardHeader className="px-4">
        <CardDescription>{label}</CardDescription>
        <CardTitle
          className={`text-2xl tabular-nums ${bad ? "text-destructive" : ""}`}
        >
          {shown}
        </CardTitle>
        {note && <CardDescription className="text-xs">{note}</CardDescription>}
      </CardHeader>
    </Card>
  );
}

function Section({
  title,
  note,
  children,
}: {
  title: string;
  note?: string;
  children: React.ReactNode;
}) {
  return (
    <Card className="min-w-0 gap-3" aria-label={title}>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        {note && <CardDescription>{note}</CardDescription>}
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  );
}

function TimeChart({
  title,
  note,
  result,
  config,
  stacked = false,
}: {
  title: string;
  note: string;
  result: Result | undefined;
  config: ChartConfig;
  stacked?: boolean;
}) {
  const data = useMemo(
    () =>
      (result?.rows ?? []).map((row) =>
        Object.fromEntries(result!.columns.map((column, i) => [column, row[i]]))
      ),
    [result]
  );
  const keys = Object.keys(config);
  const axes = [
    <CartesianGrid key="grid" vertical={false} />,
    <XAxis
      key="x"
      dataKey="t"
      tickLine={false}
      axisLine={false}
      minTickGap={32}
    />,
    <YAxis key="y" tickLine={false} axisLine={false} width={48} />,
    <ChartTooltip key="tip" content={<ChartTooltipContent />} />,
    <ChartLegend key="legend" content={<ChartLegendContent />} />,
  ];
  return (
    <Section title={title} note={note}>
      {result ? (
        <ChartContainer config={config} className="aspect-auto h-64 w-full">
          {stacked ? (
            <AreaChart data={data}>
              {axes}
              {keys.map((key) => (
                <Area
                  key={key}
                  dataKey={key}
                  type="monotone"
                  stackId="a"
                  stroke={`var(--color-${key})`}
                  fill={`var(--color-${key})`}
                  fillOpacity={0.25}
                  isAnimationActive={false}
                />
              ))}
            </AreaChart>
          ) : (
            <LineChart data={data}>
              {axes}
              {keys.map((key) => (
                <Line
                  key={key}
                  dataKey={key}
                  type="linear"
                  stroke={`var(--color-${key})`}
                  dot={false}
                  isAnimationActive={false}
                />
              ))}
            </LineChart>
          )}
        </ChartContainer>
      ) : (
        <Skeleton className="h-64" />
      )}
    </Section>
  );
}

const BADGES = new Map<string, "destructive" | "secondary" | "outline">([
  ["error", "destructive"],
  ["ERROR", "destructive"],
  ["FATAL", "destructive"],
  ["WARN", "secondary"],
  ["ok", "outline"],
]);

/** A local query's rows as a shadcn table. `trace_id` cells open the trace; status/severity cells are badges. */
function Grid({
  name,
  result,
  limit = 200,
  onRow,
  onTrace,
}: {
  name: string;
  result: Result | undefined;
  limit?: number;
  onRow?: (row: Cell[]) => void;
  onTrace?: (id: string) => void;
}) {
  if (!result) return <Skeleton className="h-32" />;
  if (result.rows.length === 0)
    return (
      <p className="text-muted-foreground" data-table={name} data-rows={0}>
        No rows match.
      </p>
    );
  const trace = result.columns.indexOf("trace_id");
  return (
    <div
      className="max-h-[32rem] overflow-auto"
      data-table={name}
      data-rows={result.rows.length}
    >
      <Table>
        <TableHeader className="bg-card sticky top-0">
          <TableRow>
            {result.columns.map((column, i) => (
              <TableHead
                key={column}
                className={result.numeric[i] ? "text-right" : ""}
              >
                {column.replaceAll("_", " ")}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {result.rows.slice(0, limit).map((row, index) => {
            const id =
              trace >= 0 && row[trace] ? String(row[trace]) : undefined;
            const click = onRow
              ? () => onRow(row)
              : id && onTrace
                ? () => onTrace(id)
                : undefined;
            return (
              <TableRow
                key={index}
                onClick={click}
                className={click ? "cursor-pointer" : ""}
              >
                {row.map((value, column) => (
                  <TableCell
                    key={column}
                    className={
                      result.numeric[column]
                        ? "text-right font-mono tabular-nums"
                        : "max-w-[40rem] truncate whitespace-nowrap"
                    }
                    title={value === null ? undefined : String(value)}
                  >
                    {value !== null && BADGES.has(String(value)) ? (
                      <Badge variant={BADGES.get(String(value))}>{value}</Badge>
                    ) : column === trace && value ? (
                      <span className="text-ring font-mono text-xs">
                        {String(value).slice(0, 12)}…
                      </span>
                    ) : result.numeric[column] ? (
                      num(value)
                    ) : (
                      (value ?? "—")
                    )}
                  </TableCell>
                ))}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}

/** The one hand-drawn view: a trace waterfall, read from the server so no span is missing. */
function TraceSheet({
  traceId,
  onClose,
}: {
  traceId: string | undefined;
  onClose: () => void;
}) {
  const [data, setData] = useState<TraceData>();
  const [error, setError] = useState<string>();
  const [picked, setPicked] = useState<string>();

  useEffect(() => {
    setData(undefined);
    setError(undefined);
    setPicked(undefined);
    const token = auth.currentToken();
    if (!traceId || !token) return;
    let live = true;
    const read = async () => {
      try {
        const result = await fetchTrace(token, traceId);
        if (live) setData(result);
      } catch (reason) {
        if (live)
          setError(reason instanceof Error ? reason.message : String(reason));
      }
    };
    void read();
    return () => {
      live = false;
    };
  }, [traceId]);

  const rows = useMemo(() => {
    if (!data || data.spans.length === 0) return [];
    const spans = data.spans;
    const ids = new Set(spans.map((span) => String(span.span_id)));
    const children = new Map<string, typeof spans>();
    for (const span of spans) {
      const parent =
        span.parent_span_id && ids.has(String(span.parent_span_id))
          ? String(span.parent_span_id)
          : "";
      children.set(parent, [...(children.get(parent) ?? []), span]);
    }
    const start = Math.min(...spans.map((span) => Number(span.start_us)));
    const end = Math.max(
      ...spans.map((span) => Number(span.start_us) + Number(span.duration))
    );
    const total = Math.max(1, end - start);
    const out: {
      span: (typeof spans)[number];
      depth: number;
      left: number;
      width: number;
    }[] = [];
    const walk = (parent: string, depth: number) => {
      for (const span of children.get(parent) ?? []) {
        out.push({
          span,
          depth,
          left: (100 * (Number(span.start_us) - start)) / total,
          width: Math.max(0.3, (100 * Number(span.duration)) / total),
        });
        walk(String(span.span_id), depth + 1);
      }
    };
    walk("", 0);
    return out;
  }, [data]);

  const chosen = data?.spans.find((span) => span.span_id === picked);
  const services = [
    ...new Set(rows.map((row) => String(row.span.service_name))),
  ];

  return (
    <Sheet
      open={traceId !== undefined}
      onOpenChange={(open) => !open && onClose()}
    >
      <SheetContent
        className="w-full overflow-y-auto sm:max-w-4xl"
        data-trace={traceId}
      >
        <SheetHeader>
          <SheetTitle>Trace</SheetTitle>
          <SheetDescription className="font-mono text-xs">
            {traceId}
            {rows.length > 0 &&
              ` · ${rows.length} spans · ${services.length} services · ${(Number(rows[0]!.span.duration) / 1000).toFixed(1)} ms root`}
          </SheetDescription>
        </SheetHeader>
        <div className="grid gap-4 px-4 pb-6">
          {error && (
            <p className="text-destructive" role="alert">
              {error}
            </p>
          )}
          {!data && !error && <Skeleton className="h-64" />}
          {data && rows.length === 0 && (
            <p className="text-muted-foreground">
              No spans stored for this trace.
            </p>
          )}
          {rows.length > 0 && (
            <div className="grid gap-px text-xs" data-waterfall={rows.length}>
              {rows.map(({ span, depth, left, width }) => (
                <button
                  key={String(span.span_id)}
                  type="button"
                  onClick={() => setPicked(String(span.span_id))}
                  className={`hover:bg-muted grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)] items-center gap-3 rounded px-1 py-1 text-left ${picked === span.span_id ? "bg-muted" : ""}`}
                >
                  <span
                    className="truncate"
                    style={{ paddingLeft: depth * 12 }}
                  >
                    <span className="text-muted-foreground">
                      {String(span.service_name)}
                    </span>{" "}
                    {String(span.span_name)}
                  </span>
                  <span className="relative h-4">
                    <span
                      className={`absolute top-0.5 h-3 rounded-sm ${span.status_code === 2 ? "bg-destructive" : "bg-ring"}`}
                      style={{ left: `${left}%`, width: `${width}%` }}
                    />
                    <span
                      className="text-muted-foreground absolute top-0 font-mono whitespace-nowrap"
                      style={{ left: `min(${left + width}% + 4px, 90%)` }}
                    >
                      {(Number(span.duration) / 1000).toFixed(1)} ms
                    </span>
                  </span>
                </button>
              ))}
            </div>
          )}
          {chosen && (
            <Section
              title={String(chosen.span_name)}
              note={String(chosen.status_message ?? "") || undefined}
            >
              <pre className="bg-muted overflow-x-auto rounded-md p-3 font-mono text-xs whitespace-pre-wrap">
                {pretty(chosen.span_attributes)}
              </pre>
            </Section>
          )}
          {data && data.logs.length > 0 && (
            <Section
              title="Logs in this trace"
              note={`${data.logs.length} lines`}
            >
              <Grid
                name="trace-logs"
                result={{
                  columns: ["time", "service", "severity", "body"],
                  numeric: [false, false, false, false],
                  rows: data.logs.map((log) => [
                    new Date(Number(log.ts_us) / 1000)
                      .toISOString()
                      .slice(11, 23),
                    log.service_name ?? null,
                    log.severity_text ?? null,
                    log.body ?? null,
                  ]),
                }}
              />
            </Section>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}

const pretty = (value: Cell | undefined) => {
  try {
    return JSON.stringify(JSON.parse(String(value)), null, 2);
  } catch {
    return String(value ?? "");
  }
};

function SqlBox({ reports }: { reports: LoadReport[] }) {
  const [sql, setSql] = useState(DEFAULT_SQL);
  const [result, setResult] = useState<Result>();
  const [error, setError] = useState<string>();
  const [running, setRunning] = useState(false);
  const run = async () => {
    setRunning(true);
    try {
      setResult(await runLocal(sql));
      setError(undefined);
    } catch (reason) {
      setResult(undefined);
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setRunning(false);
    }
  };
  return (
    <Section
      title="SQL"
      note="runs in this browser against the loaded tables · ⌘↵ to run"
    >
      <div className="grid gap-4 lg:grid-cols-[1fr_18rem]">
        <div className="grid content-start gap-2">
          <textarea
            aria-label="SQL query"
            spellCheck={false}
            className="border-input focus-visible:ring-ring/50 h-40 resize-y rounded-md border bg-transparent p-3 font-mono text-xs leading-relaxed outline-none focus-visible:ring-[3px]"
            value={sql}
            onChange={(event) => setSql(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                void run();
              }
            }}
          />
          <div>
            <Button
              data-action="run-sql"
              disabled={running}
              onClick={() => void run()}
            >
              Run
            </Button>
          </div>
        </div>
        <ul
          className="grid content-start gap-1.5 text-xs"
          aria-label="Local tables"
        >
          {reports.map((report) => (
            <li key={report.table} className="grid">
              <span className="font-mono">
                {report.table}{" "}
                <span className="text-muted-foreground tabular-nums">
                  {num(report.rows)}
                </span>
              </span>
              <span className="text-muted-foreground">
                {report.description}
              </span>
            </li>
          ))}
        </ul>
      </div>
      {error && (
        <pre
          className="text-destructive mt-3 font-mono text-xs whitespace-pre-wrap"
          role="alert"
        >
          {error}
        </pre>
      )}
      {result && (
        <div className="mt-3 grid gap-2">
          <p className="text-muted-foreground text-xs">
            {num(result.rows.length)} rows
            {result.rows.length > 500 ? " · showing 500" : ""}
          </p>
          <Grid result={result} name="sql" limit={500} />
        </div>
      )}
    </Section>
  );
}
