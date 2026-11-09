"use client";

import { Collapsible } from "@base-ui/react/collapsible";
import * as React from "react";

import { DataTableAuto } from "@/components/data-table/data-table-auto";
import { Button } from "@/components/observability/primitives";
import { RequestState } from "@/components/observability/request-state";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { query, type Row } from "@/lib/sigiro/query";

const DEFAULT_SQL = `SELECT service_name, severity_text, count(*) AS n
FROM sigiro_logs
WHERE timestamp > now() - INTERVAL 1 DAY
GROUP BY ALL
ORDER BY n DESC`;

type QueryState =
  | { kind: "idle" }
  | { kind: "running"; sql: string }
  | { kind: "error"; sql: string; message: string }
  | { kind: "ready"; sql: string; id: number; rows: Row[] };

/** SQL and authorization contracts stay on /v1/query; this owns only the UI lifecycle. */
export default function SqlPage() {
  const [sql, setSql] = React.useState(DEFAULT_SQL);
  const [state, setState] = React.useState<QueryState>({ kind: "idle" });
  const request = React.useRef<AbortController | null>(null);
  const resultId = React.useRef(0);
  const running = state.kind === "running";

  React.useEffect(() => () => request.current?.abort(), []);

  const run = async (submitted = sql) => {
    // The ref guards synchronous repeated keyboard submits, before React commits.
    if (request.current || !submitted.trim()) return;
    const controller = new AbortController();
    request.current = controller;
    setState({ kind: "running", sql: submitted });
    try {
      const rows = await query(submitted, controller.signal);
      if (request.current === controller && !controller.signal.aborted) {
        setState({
          kind: "ready",
          sql: submitted,
          id: ++resultId.current,
          rows,
        });
      }
    } catch (failure) {
      if (request.current === controller && !controller.signal.aborted) {
        setState({
          kind: "error",
          sql: submitted,
          message: failure instanceof Error ? failure.message : String(failure),
        });
      }
    } finally {
      if (request.current === controller) request.current = null;
    }
  };

  const cancel = () => {
    request.current?.abort();
    request.current = null;
    setState({ kind: "idle" });
  };

  return (
    <div
      className="flex min-h-0 flex-1 flex-col gap-3 py-4 md:py-6"
      data-page="sql"
      data-state={state.kind}
    >
      <form
        className="grid shrink-0 gap-2 px-4 lg:px-6"
        onSubmit={(event) => {
          event.preventDefault();
          void run();
        }}
      >
        <Label htmlFor="sql">SQL query</Label>
        <Textarea
          id="sql"
          className="min-h-36 font-mono text-sm"
          value={sql}
          aria-describedby="sql-help"
          onChange={(event) => setSql(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void run();
            }
          }}
        />
        <div className="flex flex-wrap items-center gap-3">
          <Button type="submit" data-action="run-sql" disabled={running || !sql.trim()}>
            {running ? "Running…" : "Run"}
          </Button>
          {running && (
            <Button type="button" onClick={cancel}>
              Cancel
            </Button>
          )}
          <span id="sql-help" className="text-muted-foreground text-xs">
            ⌘/Ctrl + Enter · tables: sigiro_spans, sigiro_logs, sigiro_metrics_histogram,
            sigiro_profiles
          </span>
        </div>
      </form>
      {state.kind === "idle" && (
        <RequestState kind="idle" title="Run a query to inspect results">
          The query defines its own time and service scope within the current workspace.
        </RequestState>
      )}
      {state.kind === "running" && (
        <RequestState kind="loading" title="Running query">
          Additional Run shortcuts are ignored until this request completes or is canceled.
        </RequestState>
      )}
      {state.kind === "error" && (
        <RequestState kind="error" title="SQL query failed" onRetry={() => run(state.sql)}>
          {state.message}
          <p>Retry executes the failed SQL. Run executes the current draft.</p>
        </RequestState>
      )}
      {(state.kind === "ready" || state.kind === "error") && (
        <Collapsible.Root className="shrink-0 space-y-2 px-4 lg:px-6">
          <div className="flex flex-wrap items-center gap-3">
            <Collapsible.Trigger render={<Button />}>Executed SQL</Collapsible.Trigger>
            {state.sql !== sql && (
              <span role="status" className="text-muted-foreground text-xs">
                Draft changed · this outcome belongs to the last executed query.
              </span>
            )}
          </div>
          <Collapsible.Panel>
            <pre
              tabIndex={0}
              aria-label="Executed SQL text"
              className="bg-muted max-h-40 overflow-auto rounded-md p-3 font-mono text-xs break-all whitespace-pre-wrap"
            >
              {state.sql}
            </pre>
          </Collapsible.Panel>
        </Collapsible.Root>
      )}
      {state.kind === "ready" && (
        <div
          className="flex min-h-0 flex-1 flex-col border-t"
          data-table="sql"
          data-rows={state.rows.length}
        >
          {state.rows.length === 0 ? (
            <RequestState kind="empty" title="No rows matched the executed query">
              Edit the SQL and run again.
            </RequestState>
          ) : (
            <DataTableAuto key={state.id} compact tableId="sql" data={state.rows} />
          )}
        </div>
      )}
    </div>
  );
}
