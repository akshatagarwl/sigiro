import * as Option from "effect/Option";
import * as Schema from "effect/Schema";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { PRIMARY, SECONDARY } from "@/components/site/action";
import { BROWSER_CLIENT_SCOPE } from "@/lib/browser-client";
import { createBrowserOAuth } from "@/lib/browser-oauth";

import { Chart, type Series } from "./chart";
import {
  type Cell,
  type LoadReport,
  QueryError,
  type Result,
  SERVER_TABLES,
  WINDOWS,
  WINDOW_KEYS,
  type Window,
  type WindowKey,
  loadWindow,
  runLocal,
  sqlString,
  windowFor,
} from "./telemetry";

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

type Filters = {
  service: string;
  minSeverity: number;
  search: string;
  range: [number, number] | null;
};

const SEVERITIES = [
  { value: 0, label: "All" },
  { value: 13, label: "Warn+" },
  { value: 17, label: "Error+" },
] as const;

const FIELD =
  "h-9 rounded-[9px] border-2 border-stone-200 bg-stone-100/70 px-2.5 text-[13px] text-ink shadow-[inset_0_1px_2px_rgba(28,25,23,0.07)] outline-none transition-[background-color,border-color,box-shadow] duration-150 focus:border-[#4568FF] focus:bg-white focus:shadow-none dark:border-white/[0.08] dark:bg-[#1D1D1A] dark:shadow-[inset_0_1px_2px_rgba(0,0,0,0.45)] dark:focus:border-[#93B0FF] dark:focus:bg-[#252522]";

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

export function Dashboard() {
  const [phase, setPhase] = useState<Phase>({ kind: "starting" });
  const [windowKey, setWindowKey] = useState<WindowKey>("24h");
  const [loaded, setLoaded] = useState<Window | null>(null);
  const [tenant, setTenant] = useState<string>();
  const [filters, setFilters] = useState<Filters>({
    service: "",
    minSeverity: 0,
    search: "",
    range: null,
  });
  const [services, setServices] = useState<string[]>([]);
  const [panels, setPanels] = useState<Record<string, Result>>({});
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
        "SELECT DISTINCT service_name FROM (SELECT service_name FROM log_buckets UNION ALL SELECT service_name FROM span_buckets) ORDER BY 1"
      );
      setServices(list.rows.map((row) => String(row[0])));
      setLoaded(w);
      setFilters((current) => ({ ...current, range: null }));
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

  const queries = useMemo(() => {
    if (!loaded) return undefined;
    const { service, minSeverity, search, range } = filters;
    const bucketSeconds = WINDOWS[loaded.key].bucketMs / 1000;
    const svc = service !== "" && `service_name = ${sqlString(service)}`;
    const inRange = (column: string) =>
      range
        ? `epoch(${column}) >= ${range[0]} AND epoch(${column}) < ${range[1]}`
        : undefined;
    const text = search.trim();
    const axis = `SELECT epoch(b) AS t FROM range(${`TIMESTAMP '${loaded.from.toISOString().slice(0, 19)}'`}, ${`TIMESTAMP '${loaded.to.toISOString().slice(0, 19)}'`}, INTERVAL ${bucketSeconds} SECOND) r(b)`;
    return {
      logsChart: `SELECT a.t, coalesce(sum(l.n), 0) AS total, coalesce(sum(l.n) FILTER (WHERE l.severity_number >= 17), 0) AS errors FROM (${axis}) a LEFT JOIN (SELECT * FROM log_buckets ${where([svc, minSeverity > 0 && `severity_number >= ${minSeverity}`])}) l ON epoch(l.bucket) = a.t GROUP BY a.t ORDER BY a.t`,
      spansChart: `SELECT a.t, coalesce(sum(s.n), 0) AS total, coalesce(sum(s.n) FILTER (WHERE s.status_code = 2), 0) AS failed FROM (${axis}) a LEFT JOIN (SELECT * FROM span_buckets ${where([svc])}) s ON epoch(s.bucket) = a.t GROUP BY a.t ORDER BY a.t`,
      templates: `SELECT coalesce(any_value(template_text), any_value(example)) AS template, service_name AS service, CASE WHEN max(severity_number) >= 21 THEN 'FATAL' WHEN max(severity_number) >= 17 THEN 'ERROR' ELSE 'WARN' END AS severity, sum(n) AS count FROM log_templates ${where([svc, minSeverity > 13 && `severity_number >= ${minSeverity}`, inRange("bucket"), text && `(${ilike("template_text", text)} OR ${ilike("example", text)})`])} GROUP BY template_id, service_name ORDER BY count DESC LIMIT 50`,
      failed: `SELECT service_name AS service, span_name AS span, status_message AS message, sum(n) AS count, any_value(trace_id) AS trace_id FROM failed_spans ${where([svc, inRange("bucket"), text && `(${ilike("span_name", text)} OR ${ilike("coalesce(status_message, '')", text)})`])} GROUP BY 1, 2, 3 ORDER BY count DESC LIMIT 50`,
      latency: `SELECT s.service_name AS service, s.span_name AS span, e.n AS spans, count(*) AS sampled, round(quantile_cont(s.duration, 0.5) / 1000, 1) AS p50_ms, round(quantile_cont(s.duration, 0.95) / 1000, 1) AS p95_ms, round(100 * avg(CASE WHEN s.status_code = 2 THEN 1 ELSE 0 END), 1) AS error_pct FROM sigiro_spans s LEFT JOIN (SELECT service_name, span_name, sum(n) AS n FROM span_buckets ${where([svc, inRange("bucket")])} GROUP BY 1, 2) e USING (service_name, span_name) ${where([svc && `s.${svc}`, inRange("s.timestamp"), text && ilike("s.span_name", text)])} GROUP BY 1, 2, 3 ORDER BY p95_ms DESC LIMIT 50`,
      logs: `SELECT timestamp, service_name AS service, severity_text AS severity, body FROM sigiro_logs ${where([svc, minSeverity > 0 && `severity_number >= ${minSeverity}`, inRange("timestamp"), text && ilike("body", text)])} ORDER BY timestamp DESC LIMIT 100`,
    };
  }, [loaded, filters]);

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

  const charts = useMemo(() => {
    const series = (result: Result | undefined, labels: [string, string]) => {
      const rows = result?.rows ?? [];
      const times = rows.map((row) => Number(row[0]));
      const lines: Series[] = [
        {
          label: labels[0],
          values: rows.map((row) => Number(row[1])),
          tone: "ink",
        },
        {
          label: labels[1],
          values: rows.map((row) => Number(row[2])),
          tone: "flag",
        },
      ];
      return { times, lines };
    };
    return {
      logs: series(panels.logsChart, ["Logs", "Errors"]),
      spans: series(panels.spansChart, ["Spans", "Failed"]),
    };
  }, [panels.logsChart, panels.spansChart]);

  if (phase.kind === "starting")
    return (
      <Shell>
        <Status>Checking sign-in…</Status>
      </Shell>
    );

  if (phase.kind === "signed-out")
    return (
      <Shell>
        <section
          className="mat-panel mx-auto grid max-w-[30rem] gap-4 rounded-[14px] p-6"
          data-state="signed-out"
        >
          <h2 className="text-ink text-[15px] font-medium">
            Sign in to read your telemetry
          </h2>
          <p className="text-ink-2 text-[13.5px] leading-relaxed">
            The dashboard reads your tenant through the query API with a
            read-only token. The token stays in this tab and expires after 15
            minutes.
          </p>
          {phase.reason && (
            <p
              className="text-[13px] text-red-600 dark:text-red-400"
              role="alert"
            >
              {phase.reason}
            </p>
          )}
          <div>
            <button
              type="button"
              className={PRIMARY}
              data-action="sign-in"
              onClick={() => {
                auth.beginAuthorization().catch(() => undefined);
              }}
            >
              Sign in
            </button>
          </div>
        </section>
      </Shell>
    );

  if (phase.kind === "error")
    return (
      <Shell>
        <section
          className="mat-panel grid gap-3 rounded-[14px] p-6"
          data-state="error"
          role="alert"
        >
          <h2 className="text-ink text-[15px] font-medium">
            The dashboard could not load
          </h2>
          <pre className="mat-well text-ink-2 overflow-x-auto rounded-[9px] p-3 font-mono text-[12px] whitespace-pre-wrap">
            {phase.message}
          </pre>
          <div>
            <button
              type="button"
              className={SECONDARY}
              onClick={() => void load(windowKey)}
            >
              Try again
            </button>
          </div>
        </section>
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

  return (
    <Shell>
      <div className="grid gap-4" data-state={phase.kind}>
        <header className="flex flex-wrap items-center gap-3">
          <div
            role="radiogroup"
            aria-label="Time window"
            className="mat-well flex rounded-[12px] p-1"
          >
            {WINDOW_KEYS.map((key) => (
              <button
                key={key}
                type="button"
                role="radio"
                aria-checked={key === windowKey}
                className={`h-7 rounded-[8px] px-3 font-mono text-[12px] tabular-nums transition-colors duration-150 ${key === windowKey ? "bg-stone-800 text-white dark:bg-stone-100 dark:text-stone-900" : "text-ink-2 hover:text-ink"}`}
                onClick={() => {
                  if (loading) return;
                  setWindowKey(key);
                  void load(key);
                }}
              >
                {key}
              </button>
            ))}
          </div>
          <button
            type="button"
            className={SECONDARY}
            aria-disabled={loading}
            onClick={() => !loading && void load(windowKey)}
          >
            Reload
          </button>
          <p className="meta text-ink-3 ml-auto" data-window>
            {loaded
              ? `${formatUtc(loaded.from)} → ${formatUtc(loaded.to)}`
              : "—"}
            {tenant ? ` · tenant ${tenant}` : ""}
          </p>
        </header>

        <Progress reports={reports} loading={loading} />

        {truncated.length > 0 && (
          <p className="text-ink-2 text-[12.5px]" role="status">
            The server capped{" "}
            {truncated.map((report) => report.table).join(", ")} at its row
            limit; those tables are incomplete.
          </p>
        )}

        {empty && (
          <section className="mat-panel rounded-[14px] p-6" data-state="empty">
            <h2 className="text-ink text-[15px] font-medium">
              No telemetry in this window
            </h2>
            <p className="text-ink-2 mt-2 text-[13.5px] leading-relaxed">
              Tenant{" "}
              <code className="font-mono text-[12px]">
                {tenant ?? "unknown"}
              </code>{" "}
              has no logs or spans between{" "}
              {loaded ? formatUtc(loaded.from) : "—"} and{" "}
              {loaded ? formatUtc(loaded.to) : "—"}. Pick a longer window, or
              check that your services export to this tenant.
            </p>
          </section>
        )}

        {!loading && !empty && (
          <>
            <section
              className="flex flex-wrap items-center gap-2"
              aria-label="Filters"
            >
              <select
                aria-label="Service"
                className={FIELD}
                value={filters.service}
                onChange={(event) =>
                  setFilters({ ...filters, service: event.target.value })
                }
              >
                <option value="">All services</option>
                {services.map((service) => (
                  <option key={service} value={service}>
                    {service}
                  </option>
                ))}
              </select>
              <select
                aria-label="Minimum severity"
                className={FIELD}
                value={filters.minSeverity}
                onChange={(event) =>
                  setFilters({
                    ...filters,
                    minSeverity: Number(event.target.value),
                  })
                }
              >
                {SEVERITIES.map((severity) => (
                  <option key={severity.value} value={severity.value}>
                    {severity.label}
                  </option>
                ))}
              </select>
              <input
                type="search"
                aria-label="Search"
                placeholder="Search messages and spans"
                className={`${FIELD} min-w-[14rem] flex-1`}
                value={filters.search}
                onChange={(event) =>
                  setFilters({ ...filters, search: event.target.value })
                }
              />
              {filters.range && (
                <button
                  type="button"
                  className={SECONDARY}
                  onClick={() => setFilters({ ...filters, range: null })}
                >
                  Clear time selection
                </button>
              )}
            </section>
            {panelError && (
              <p
                className="text-[13px] text-red-600 dark:text-red-400"
                role="alert"
              >
                {panelError}
              </p>
            )}
            <div className="grid gap-4 lg:grid-cols-2">
              <Panel
                title="Logs"
                note={`per ${WINDOWS[windowKey].bucket} · drag to select a time range`}
              >
                <Chart
                  label="Logs"
                  times={charts.logs.times}
                  series={charts.logs.lines}
                  selection={filters.range}
                  onSelect={(range) =>
                    setFilters((current) => ({ ...current, range }))
                  }
                />
              </Panel>
              <Panel
                title="Spans"
                note={`per ${WINDOWS[windowKey].bucket} · drag to select a time range`}
              >
                <Chart
                  label="Spans"
                  times={charts.spans.times}
                  series={charts.spans.lines}
                  selection={filters.range}
                  onSelect={(range) =>
                    setFilters((current) => ({ ...current, range }))
                  }
                />
              </Panel>
            </div>
            <Panel title="Top warning and error templates" note="exact counts">
              <DataTable result={panels.templates} name="templates" />
            </Panel>
            <Panel title="Failed spans" note="exact counts">
              <DataTable result={panels.failed} name="failed" />
            </Panel>
            <Panel
              title="Latency by span"
              note="p50 and p95 from the span sample"
            >
              <DataTable result={panels.latency} name="latency" />
            </Panel>
            <Panel title="Recent logs" note={`newest 100 of the loaded rows`}>
              <DataTable result={panels.logs} name="logs" />
            </Panel>
          </>
        )}

        {!loading && <SqlBox reports={reports} />}
      </div>
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return <div className="w-full px-4 py-8 sm:px-6">{children}</div>;
}

function Status({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-ink-2 text-[13.5px]" role="status">
      {children}
    </p>
  );
}

function Panel({
  title,
  note,
  children,
}: {
  title: string;
  note?: string;
  children: React.ReactNode;
}) {
  return (
    <section
      className="mat-panel min-w-0 rounded-[14px] p-4"
      aria-label={title}
    >
      <header className="mb-3 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h2 className="text-ink text-[13px] font-medium">{title}</h2>
        {note && <p className="meta text-ink-3">{note}</p>}
      </header>
      {children}
    </section>
  );
}

function Progress({
  reports,
  loading,
}: {
  reports: LoadReport[];
  loading: boolean;
}) {
  return (
    <section aria-label="Loaded tables" className="grid gap-2">
      <div className="flex items-center gap-3">
        <div className="flex gap-[2px]" aria-hidden>
          {SERVER_TABLES.map((table, index) => (
            <span
              key={table.name}
              className={`size-[5px] rounded-[1.5px] ${index < reports.length ? "bg-accent" : "bg-hairline"}`}
            />
          ))}
        </div>
        <p
          className="meta text-ink-3"
          role="status"
          data-progress={reports.length}
        >
          {loading
            ? `Loading ${reports.length + 1} of ${SERVER_TABLES.length}: ${SERVER_TABLES[reports.length]?.name ?? ""}`
            : `${reports.length} tables in DuckDB-WASM`}
        </p>
      </div>
    </section>
  );
}

const render = (value: Cell, numeric: boolean) =>
  value === null
    ? "—"
    : numeric
      ? Number(value).toLocaleString("en")
      : String(value);

function DataTable({
  result,
  name,
  limit = 100,
}: {
  result: Result | undefined;
  name: string;
  limit?: number;
}) {
  if (!result) return <Status>Querying…</Status>;
  if (result.rows.length === 0)
    return (
      <p className="text-ink-3 text-[13px]" data-table={name} data-rows={0}>
        No rows match.
      </p>
    );
  return (
    <div
      className="mat-well max-h-[26rem] overflow-auto rounded-[9px]"
      data-table={name}
      data-rows={result.rows.length}
    >
      <table className="w-full min-w-[40rem] border-collapse text-left text-[12.5px]">
        <thead className="bg-well sticky top-0">
          <tr>
            {result.columns.map((column) => (
              <th
                key={column}
                scope="col"
                className="label border-hairline border-b px-3 py-2 font-semibold whitespace-nowrap"
              >
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {result.rows.slice(0, limit).map((row, index) => (
            <tr
              key={index}
              className="border-hairline border-b last:border-b-0"
            >
              {row.map((value, column) => (
                <td
                  key={column}
                  className={`text-ink-2 max-w-[42rem] px-3 py-1.5 align-top break-words ${result.numeric[column] ? "text-right font-mono tabular-nums" : ""}`}
                >
                  {render(value, result.numeric[column] ?? false)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

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
    <Panel
      title="SQL"
      note="runs in this browser against the loaded tables · ⌘↵ to run"
    >
      <div className="grid gap-3 lg:grid-cols-[1fr_18rem]">
        <div className="grid content-start gap-2">
          <textarea
            aria-label="SQL query"
            spellCheck={false}
            className={`${FIELD.replace("h-9 ", "")} h-36 resize-y py-2 font-mono text-[12.5px] leading-relaxed`}
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
            <button
              type="button"
              className={PRIMARY}
              aria-disabled={running}
              data-action="run-sql"
              onClick={() => !running && void run()}
            >
              Run
            </button>
          </div>
        </div>
        <ul
          className="grid content-start gap-1.5 text-[12px]"
          aria-label="Local tables"
        >
          {reports.map((report) => (
            <li key={report.table} className="grid">
              <span className="text-ink font-mono">
                {report.table}{" "}
                <span className="text-ink-3 tabular-nums">
                  {report.rows.toLocaleString("en")}
                </span>
              </span>
              <span className="text-ink-3">{report.description}</span>
            </li>
          ))}
        </ul>
      </div>
      {error && (
        <pre
          className="mt-3 font-mono text-[12px] whitespace-pre-wrap text-red-600 dark:text-red-400"
          role="alert"
        >
          {error}
        </pre>
      )}
      {result && (
        <div className="mt-3">
          <p className="meta text-ink-3 mb-2">
            {result.rows.length.toLocaleString("en")} rows
            {result.rows.length > 500 ? " · showing 500" : ""}
          </p>
          <DataTable result={result} name="sql" limit={500} />
        </div>
      )}
    </Panel>
  );
}
