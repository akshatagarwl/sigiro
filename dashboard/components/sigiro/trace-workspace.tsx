"use client";

import { useVirtualizer } from "@tanstack/react-virtual";
import { CircleAlert, ChevronDown, ChevronRight, Minus, Plus, Search } from "lucide-react";
import { useQueryState, parseAsString } from "nuqs";
import { useMemo, useRef, useState } from "react";

import { MAX_ZOOM, useWheelZoom } from "@/components/laminar/use-wheel-zoom";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { useIsMobile } from "@/hooks/use-mobile";
import { type Row } from "@/lib/sigiro/query";
import {
  durationLabel,
  parsePayload,
  traceModel,
  type Span,
} from "@/lib/sigiro/trace";

function Payload({ value }: { value: unknown }) {
  const parsed = parsePayload(value);
  return (
    <pre className="bg-muted/40 overflow-auto rounded-md p-3 font-mono text-xs leading-6 break-words whitespace-pre-wrap">
      {typeof parsed === "string" ? parsed : JSON.stringify(parsed, null, 2)}
    </pre>
  );
}

function SpanInspector({
  span,
  traceId,
  traceWindow,
}: {
  span: Span;
  traceId: string;
  traceWindow: { from: number; to: number };
}) {
  const attributes = parsePayload(span.row.span_attributes);
  const entries =
    attributes && typeof attributes === "object" && !Array.isArray(attributes)
      ? Object.entries(attributes)
      : [];
  const isOutput = (key: string) => /(?:output|completion|response)/i.test(key);
  const input = entries.filter(
    ([key]) => !isOutput(key) && /(?:input|prompt|request|messages)/i.test(key)
  );
  const output = entries.filter(([key]) => isOutput(key));
  const payloads = [
    { title: "Input", entries: input },
    { title: "Output", entries: output },
  ];
  const from = Math.floor(span.start / 1000) - 60_000;
  const to = Math.ceil((span.start + span.duration) / 1000) + 60_000;
  return (
    <aside
      className="bg-background h-full min-w-0 overflow-auto p-4 sm:p-5"
      aria-label="Span inspector"
    >
      <div className="mb-4 space-y-2">
        <div className="flex items-center gap-2">
          <Badge variant={span.error ? "destructive" : "secondary"}>
            {span.error
              ? "Error"
              : Number(span.row.status_code) === 1
                ? "OK"
                : "Unset"}
          </Badge>
        </div>
        <h3 className="text-base font-semibold break-words">{span.name}</h3>
        <div className="text-muted-foreground font-mono text-xs break-all">{span.id}</div>
        <dl className="bg-muted/30 grid grid-cols-2 gap-4 rounded-md border p-3">
          <div>
            <dt className="text-muted-foreground text-xs">Duration</dt>
            <dd className="mt-1 text-sm font-medium tabular-nums">{durationLabel(span.duration)}</dd>
          </div>
          <div>
            <dt className="text-muted-foreground text-xs">Service</dt>
            <dd className="mt-1 text-sm font-medium break-words">{span.service}</dd>
          </div>
        </dl>
        {span.row.status_message ? (
          <p
            role={span.error ? "alert" : undefined}
            className="border-destructive/30 text-destructive rounded border p-2 text-sm break-words"
          >
            {String(span.row.status_message)}
          </p>
        ) : null}
        <div className="flex flex-wrap gap-2">
          <Button asChild variant="outline" size="sm">
            <a
              href={`/dashboard/logs/?trace_id=${traceId}&timestamp=${traceWindow.from}-${traceWindow.to}`}
            >
              Trace logs
            </a>
          </Button>
          <Button asChild variant="outline" size="sm">
            <a
              href={`/dashboard/profiles/?service=${encodeURIComponent(span.service)}&ptype=cpu&trace=${traceId}&span=${span.id}&from=${from}&to=${to}`}
            >
              Span profile
            </a>
          </Button>
        </div>
      </div>
      <Tabs defaultValue="io" key={span.id}>
        <TabsList className="w-full">
          <TabsTrigger value="io">Input / output</TabsTrigger>
          <TabsTrigger value="events">Events</TabsTrigger>
          <TabsTrigger value="attributes">Attributes</TabsTrigger>
        </TabsList>
        <TabsContent value="io" className="space-y-3">
          {payloads.map(({ title, entries }) => (
            <section key={title}>
              <h4 className="my-2 text-sm font-medium">{title}</h4>
              {entries.length ? (
                entries.map(([key, value]) => (
                  <div className="mb-3" key={key}>
                    <div className="text-muted-foreground mb-1 text-xs break-all">
                      {key}
                    </div>
                    <Payload value={value} />
                  </div>
                ))
              ) : (
                <p className="text-muted-foreground text-sm">
                  No {title.toLowerCase()} attributes recorded.
                </p>
              )}
            </section>
          ))}
        </TabsContent>
        <TabsContent value="events">
          <Payload value={span.row.events_json} />
        </TabsContent>
        <TabsContent value="attributes" className="space-y-3">
          <h4 className="text-sm font-medium">Span</h4>
          <Payload value={attributes} />
          <h4 className="text-sm font-medium">Resource</h4>
          <Payload value={span.row.resource_attributes} />
          <h4 className="text-sm font-medium">Links</h4>
          <Payload value={span.row.links_json} />
        </TabsContent>
      </Tabs>
    </aside>
  );
}

export function TraceWorkspace({
  rows,
  traceId,
  truncated,
}: {
  rows: Row[];
  traceId: string;
  truncated: boolean;
}) {
  const model = useMemo(() => traceModel(rows), [rows]);
  const [selectedId, setSelectedId] = useQueryState(
    "span",
    parseAsString.withOptions({ history: "replace", shallow: true })
  );
  const [search, setSearch] = useState("");
  const [errorsOnly, setErrorsOnly] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [zoom, setZoom] = useState(1);
  const isMobile = useIsMobile();
  const scrollRef = useRef<HTMLDivElement>(null);
  useWheelZoom(scrollRef, zoom, setZoom);
  const selected = model.byId.get(selectedId ?? "") ?? model.spans[0];
  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    let hiddenDepth = Infinity;
    return model.ordered.filter(({ span, depth }) => {
      if (needle || errorsOnly)
        return (
          (!errorsOnly || span.error) &&
          (!needle ||
            `${span.name} ${span.service} ${span.id}`
              .toLowerCase()
              .includes(needle))
        );
      if (depth <= hiddenDepth) hiddenDepth = Infinity;
      if (depth > hiddenDepth) return false;
      if (collapsed.has(span.id)) hiddenDepth = depth;
      return true;
    });
  }, [model, search, errorsOnly, collapsed]);
  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 36,
    overscan: 12,
    paddingStart: 28,
  });
  const select = (span: Span) => {
    void setSelectedId(span.id);
  };
  const toggle = (id: string) =>
    setCollapsed((old) => {
      const next = new Set(old);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <div
      className="trace-workspace flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border"
      data-trace-workspace={rows.length}
    >
      <div className="bg-muted/20 flex flex-wrap items-center gap-3 border-b px-4 py-3">
        <Badge variant="outline">
          {rows.length}
          {truncated ? "+" : ""} spans
        </Badge>
        <span className="text-sm font-medium">
          {durationLabel(model.duration)}
        </span>
        <span className="text-muted-foreground text-xs">
          {new Set(model.spans.map((span) => span.service)).size} services
        </span>
        <span className="text-destructive inline-flex items-center gap-1.5 text-xs font-medium">
          <CircleAlert className="size-3.5" aria-hidden="true" />
          {model.spans.filter((span) => span.error).length} errors
        </span>
        <span className="text-muted-foreground ml-auto hidden text-xs sm:inline">
          Drag the divider to resize
        </span>
      </div>
      {selectedId && !model.byId.has(selectedId) && (
        <p className="border-b bg-amber-500/10 px-4 py-2 text-sm" role="status">
          The linked span is not in the loaded spans. Showing the first
          available span.
        </p>
      )}
      {truncated && (
        <p className="border-b bg-amber-500/10 px-4 py-2 text-sm" role="status">
          Showing the first 2,000 spans. Timeline and totals describe this
          subset, not the complete trace.
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2 border-b p-2">
        <div className="relative min-w-40 flex-1">
          <Search className="text-muted-foreground absolute top-2.5 left-2 size-4" />
          <Input
            aria-label="Find spans"
            placeholder="Find spans by name, service or ID…"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            className="pl-8"
          />
        </div>
        <Button
          variant={errorsOnly ? "secondary" : "outline"}
          size="sm"
          aria-pressed={errorsOnly}
          onClick={() => setErrorsOnly(!errorsOnly)}
        >
          Errors only
        </Button>
        <div className="flex shrink-0 items-center gap-1" role="group" aria-label="Timeline zoom">
        <Button
          size="icon"
          variant="ghost"
          aria-label="Zoom out"
          disabled={zoom <= 1}
          onClick={() => setZoom(Math.max(1, zoom - 0.5))}
        >
          <Minus className="size-4" />
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setZoom(1);
            if (scrollRef.current) scrollRef.current.scrollLeft = 0;
          }}
          aria-label="Reset zoom"
        >
          {zoom.toFixed(1)}×
        </Button>
        <Button
          size="icon"
          variant="ghost"
          aria-label="Zoom in"
          disabled={zoom >= MAX_ZOOM}
          onClick={() => setZoom(Math.min(MAX_ZOOM, zoom + 0.5))}
        >
          <Plus className="size-4" />
        </Button>
        </div>
      </div>
      <ResizablePanelGroup
        orientation={isMobile ? "vertical" : "horizontal"}
        className="h-[65vh]! min-h-96"
      >
        <ResizablePanel
          defaultSize="65%"
          minSize="30%"
          className="flex min-w-0 flex-col"
        >
          <div
            ref={scrollRef}
            tabIndex={0}
            className="focus-visible:outline-ring relative min-h-0 flex-1 overflow-auto focus-visible:outline"
            aria-label="Span tree and timeline. Use arrow keys to select spans."
            onKeyDown={(event) => {
              if (
                event.target !== event.currentTarget ||
                !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)
              )
                return;
              event.preventDefault();
              const current = filtered.findIndex(
                ({ span }) => span.id === selected.id
              );
              const index =
                event.key === "Home"
                  ? 0
                  : event.key === "End"
                    ? filtered.length - 1
                    : Math.max(
                        0,
                        Math.min(
                          filtered.length - 1,
                          current + (event.key === "ArrowDown" ? 1 : -1)
                        )
                      );
              if (filtered[index]) {
                select(filtered[index].span);
                virtualizer.scrollToIndex(index);
              }
            }}
          >
            <div
              style={{
                width: `${zoom * 100}%`,
                minWidth: 560,
                height: Math.max(virtualizer.getTotalSize(), 100),
              }}
              className="relative"
            >
              <div className="bg-muted text-muted-foreground sticky top-0 z-20 flex h-7 border-b text-[11px] font-medium tabular-nums">
                <div className="bg-background sticky left-0 z-20 w-64 shrink-0 border-r px-3 py-1">
                  Span · service
                </div>
                <div className="flex flex-1 justify-between px-2 py-1">
                  {[0, 0.25, 0.5, 0.75, 1].map((fraction) => (
                    <span key={fraction}>
                      {durationLabel(model.duration * fraction)}
                    </span>
                  ))}
                </div>
              </div>
              {virtualizer.getVirtualItems().map((item) => {
                const { span, depth, children } = filtered[item.index];
                return (
                  <div
                    key={span.id}
                    className={`absolute flex h-9 w-full border-b text-xs ${selected.id === span.id ? "bg-accent" : "hover:bg-muted/50"}`}
                    data-selected={selected.id === span.id}
                    style={{ top: item.start }}
                    data-span-row={span.id}
                  >
                    <div
                      className={`sticky left-0 z-10 flex w-64 shrink-0 items-center border-r ${selected.id === span.id ? "bg-accent" : "bg-background"}`}
                      style={{ paddingLeft: Math.min(depth, 10) * 12 + 4 }}
                    >
                      {children ? (
                        <button
                          className="hover:bg-muted flex size-6 shrink-0 items-center justify-center rounded"
                          aria-label={`${collapsed.has(span.id) ? "Expand" : "Collapse"} ${span.name}`}
                          aria-expanded={!collapsed.has(span.id)}
                          onClick={() => toggle(span.id)}
                        >
                          {collapsed.has(span.id) ? (
                            <ChevronRight className="size-3" />
                          ) : (
                            <ChevronDown className="size-3" />
                          )}
                        </button>
                      ) : (
                        <span className="w-5 shrink-0" />
                      )}
                      <button
                        className="min-w-0 flex-1 truncate py-2 pr-2 text-left"
                        title={`${span.name} · ${span.service}`}
                        aria-pressed={selected.id === span.id}
                        onClick={() => select(span)}
                      >
                        {span.error && <CircleAlert className="text-destructive mr-1 inline size-3.5" aria-label="Error" />}
                        <span className={span.error ? "text-destructive font-medium" : ""}>
                          {span.name}
                        </span>
                        <span className="text-muted-foreground ml-2">
                          {span.service}
                        </span>
                      </button>
                    </div>
                    <button
                      className="relative min-w-0 flex-1 text-left"
                      aria-label={`Inspect ${span.name}, ${durationLabel(span.duration)}`}
                      onClick={() => select(span)}
                    >
                      {[0.25, 0.5, 0.75].map((fraction) => (
                        <span
                          key={fraction}
                          className="absolute inset-y-0 border-l border-dashed"
                          style={{ left: `${fraction * 100}%` }}
                        />
                      ))}
                      <span
                        className={`absolute top-2 h-5 min-w-0.5 rounded-sm ${span.error ? "bg-destructive/75" : "bg-primary/60"}`}
                        style={{
                          left: `${((span.start - model.start) / model.duration) * 100}%`,
                          width: `${(span.duration / model.duration) * 100}%`,
                        }}
                      />
                      <span className="bg-background/80 absolute top-2 right-1 rounded px-1 text-[10px] tabular-nums">
                        {durationLabel(span.duration)}
                      </span>
                    </button>
                  </div>
                );
              })}
              {filtered.length === 0 && (
                <p className="text-muted-foreground p-8 text-sm">
                  No spans match these filters.
                </p>
              )}
            </div>
          </div>
          <div className="text-muted-foreground border-t px-3 py-2 text-xs"
            role="status"
            aria-live="polite"
            aria-atomic="true">
            {filtered.length} matching spans · Ctrl/⌘ + scroll to zoom
          </div>
        </ResizablePanel>
        <ResizableHandle
          withHandle
          aria-label="Resize timeline and inspector"
        />
        <ResizablePanel minSize="25%">
          <SpanInspector
            key={selected.id}
            span={selected}
            traceId={traceId}
            traceWindow={{
              from: Math.floor(model.start / 1000) - 60_000,
              to: Math.ceil((model.start + model.duration) / 1000) + 60_000,
            }}
          />
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}
