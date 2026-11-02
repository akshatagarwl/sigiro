"use client";

import * as React from "react";

import { DataTableAuto } from "@/components/data-table/data-table-auto";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { query, type Row } from "@/lib/sigiro/query";

const DEFAULT_SQL = `SELECT service_name, severity_text, count(*) AS n
FROM sigiro_logs
WHERE timestamp > now() - INTERVAL 1 DAY
GROUP BY ALL
ORDER BY n DESC`;

/** Read-only SQL against the workspace through `/v1/query`; results in the openstatus auto table. */
export default function SqlPage() {
  const [sql, setSql] = React.useState(DEFAULT_SQL);
  const [result, setResult] = React.useState<{ id: number; rows: Row[] }>();
  const [error, setError] = React.useState<string>();
  const [running, setRunning] = React.useState(false);

  const run = async () => {
    setRunning(true);
    setError(undefined);
    try {
      const rows = await query(sql);
      setResult((previous) => ({ id: (previous?.id ?? 0) + 1, rows }));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 py-4 md:py-6">
      <form
        className="grid gap-2 px-4 lg:px-6"
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
          onChange={(event) => setSql(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void run();
            }
          }}
        />
        <div className="flex items-center gap-3">
          <Button type="submit" data-action="run-sql" disabled={running}>
            {running ? "Running…" : "Run"}
          </Button>
          <span className="text-muted-foreground text-xs">
            ⌘/Ctrl + Enter · tables: sigiro_spans, sigiro_logs,
            sigiro_metrics_histogram, sigiro_profiles
          </span>
        </div>
        {error && (
          <p className="text-destructive text-sm" role="alert">
            {error}
          </p>
        )}
      </form>
      {result && (
        <div
          className="flex min-h-0 flex-1 flex-col border-t"
          data-table="sql"
          data-rows={result.rows.length}
        >
          {result.rows.length === 0 ? (
            <p className="text-muted-foreground p-6 text-sm">No rows.</p>
          ) : (
            <DataTableAuto key={result.id} data={result.rows} />
          )}
        </div>
      )}
    </div>
  );
}
