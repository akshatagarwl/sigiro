"use client";

import dynamic from "next/dynamic";
import Link from "next/link";
import * as React from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import styles from "./log-viewer.module.css";

const LazyLog = dynamic(
  () => import("@melloware/react-logviewer").then((module) => module.LazyLog),
  { ssr: false },
);

export type LogEntry = {
  uuid: string;
  timestamp: Date;
  service: string;
  severity: string;
  level: string;
  body: string;
  attributes: string;
  trace_id: string;
};

export function LogViewer({ entries }: { entries: LogEntry[] }) {
  const [find, setFind] = React.useState("");
  const [wrap, setWrap] = React.useState(true);
  const [follow, setFollow] = React.useState(false);
  const [selectedId, setSelectedId] = React.useState<string>();
  const [notice, setNotice] = React.useState("");
  const visible = React.useMemo(() => {
    const needle = find.toLocaleLowerCase();
    return entries.filter((entry) =>
      `${entry.service} ${entry.severity} ${entry.body}`.toLocaleLowerCase().includes(needle),
    );
  }, [entries, find]);
  const selected = visible.find((entry) => entry.uuid === selectedId);
  const text = React.useMemo(
    () => visible.map((entry) => `${entry.timestamp.toISOString()} ${JSON.stringify(entry.severity || entry.level)} ${JSON.stringify(entry.service)} ${JSON.stringify(entry.body)}`).join("\n"),
    [visible],
  );
  const inspect = (index: number) => {
    const entry = visible[Math.max(0, Math.min(index, visible.length - 1))];
    if (entry) setSelectedId(entry.uuid);
  };
  return (
    <section aria-label="Log console" className="min-w-0 space-y-3 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <Input aria-label="Find in loaded logs" placeholder="Find in loaded logs" value={find} onChange={(event) => setFind(event.target.value)} className="w-full sm:w-64" />
        <Button variant="outline" size="sm" aria-pressed={wrap} onClick={() => setWrap(!wrap)}>Wrap</Button>
        <Button variant="outline" size="sm" aria-pressed={follow} onClick={() => setFollow(!follow)}>Follow loaded rows</Button>
        <Button variant="outline" size="sm" disabled={!visible.length} onClick={() => { void navigator.clipboard.writeText(text).then(() => setNotice("Copied loaded logs"), () => setNotice("Copy failed. Select text to copy manually.")); }}>Copy loaded</Button>
        <span role="status" className="text-muted-foreground text-xs">{visible.length} / {entries.length} loaded · {notice}</span>
      </div>
      <p className="text-muted-foreground text-xs">Find searches loaded rows only. Load more below. Line numbers inspect an event; Arrow keys change selection. Follow does not enable live polling.</p>
      <div role="region" aria-label="Loaded log lines" tabIndex={0} className="rounded-md border focus-visible:outline-2 focus-visible:outline-ring" onKeyDown={(event) => {
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        const index = visible.findIndex((entry) => entry.uuid === selectedId);
        inspect(index + (event.key === "ArrowDown" ? 1 : -1));
      }}>
        {visible.length ? <LazyLog
          text={text}
          className={styles.console}
          height={420}
          enableLinks={false}
          enableSearch={false}
          enableHotKeys={false}
          enableMultilineHighlight={false}
          selectableLines
          wrapLines={wrap}
          follow={follow}
          scrollToLine={follow ? undefined : selected ? visible.indexOf(selected) + 1 : undefined}
          onLineNumberClick={({ lineNumber }) => inspect(lineNumber - 1)}
          style={{ background: "var(--background)", color: "var(--foreground)", fontSize: 12 }}
          overscanRowCount={10}
        /> : <p className="p-8 text-sm">No loaded logs match.</p>}
      </div>
      {selected && <div className="min-w-0 rounded-md border p-3">
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2 text-sm"><strong>{selected.service} · {selected.severity || selected.level}</strong><Button size="sm" variant="ghost" onClick={() => setSelectedId(undefined)}>Close event</Button></div>
        <pre tabIndex={0} aria-label="Event JSON" className="max-h-72 overflow-auto whitespace-pre-wrap break-all text-xs">{JSON.stringify({ ...selected, attributes: parseAttributes(selected.attributes) }, null, 2)}</pre>
        {selected.trace_id && <Link className="mt-2 inline-block text-sm underline" href={`/traces/?trace=${encodeURIComponent(selected.trace_id)}`}>View trace</Link>}
      </div>}
    </section>
  );
}

function parseAttributes(attributes: string): unknown {
  try { return JSON.parse(attributes); } catch { return attributes; }
}
