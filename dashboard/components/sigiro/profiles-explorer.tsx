"use client";

import { IconFlame, IconX } from "@tabler/icons-react";
import { useQuery } from "@tanstack/react-query";
import {
  parseAsArrayOf,
  parseAsInteger,
  parseAsString,
  parseAsStringLiteral,
  useQueryState,
} from "nuqs";
import * as React from "react";

import { EvilBarChart } from "@/components/evilcharts/charts/recharts-bar-chart";
import type { ChartConfig } from "@/components/evilcharts/ui/recharts-chart";
import { Flamegraph } from "@/components/sigiro/flamegraph";
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
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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
import { useControls } from "@/lib/sigiro/controls";
import {
  buildDiffTree,
  buildTree,
  formatValue,
  isSnapshot,
  labelsSql,
  percent,
  servicesSql,
  stacksSql,
  summarySql,
  timelineSql,
  toStacks,
  topFunctions,
  typesSql,
  TYPE_LABELS,
  type LabelFilter,
  type ProfileQuery,
} from "@/lib/sigiro/profiles";
import { bucketFor, num, query } from "@/lib/sigiro/query";

const COMPARE = ["off", "previous", "1h", "1d"] as const;
type Compare = (typeof COMPARE)[number];
const COMPARE_LABELS: Record<Compare, string> = {
  off: "No comparison",
  previous: "vs previous window",
  "1h": "vs 1 hour earlier",
  "1d": "vs 1 day earlier",
};
const VIEWS = ["both", "flamegraph", "table"] as const;

const parseLabel = (s: string): LabelFilter | undefined => {
  const i = s.indexOf("=");
  return i > 0 ? { key: s.slice(0, i), value: s.slice(i + 1) } : undefined;
};

function useSqlQuery(name: string, sql: string | undefined) {
  return useQuery({
    queryKey: [name, sql],
    enabled: Boolean(sql),
    queryFn: ({ signal }) => query(sql!, signal),
    placeholderData: (previous) => previous,
    retry: false,
  });
}

/**
 * Pyroscope-style profile explorer over `sigiro_profiles`: pick a service and
 * profile type, narrow by sample labels or a trace/span, and read the result
 * as a flame graph and a top-functions table, optionally diffed against an
 * earlier window.
 */
export function ProfilesExplorer() {
  const controls = useControls();
  const [service, setService] = useQueryState("service", parseAsString.withDefault(""));
  const [type, setType] = useQueryState("ptype", parseAsString.withDefault(""));
  const [labelParams, setLabelParams] = useQueryState(
    "label",
    parseAsArrayOf(parseAsString, ",").withDefault([])
  );
  const [traceId, setTraceId] = useQueryState("trace", parseAsString.withDefault(""));
  const [spanId, setSpanId] = useQueryState("span", parseAsString.withDefault(""));
  const [fromMs, setFromMs] = useQueryState("from", parseAsInteger);
  const [toMs, setToMs] = useQueryState("to", parseAsInteger);
  const [compare, setCompare] = useQueryState(
    "compare",
    parseAsStringLiteral(COMPARE).withDefault("off")
  );
  const [view, setView] = useQueryState("view", parseAsStringLiteral(VIEWS).withDefault("both"));
  const [search, setSearch] = React.useState("");

  const custom = fromMs !== null && toMs !== null && toMs > fromMs;
  const from = custom ? new Date(fromMs) : controls.from;
  const to = custom ? new Date(toMs) : controls.to;
  const labels = labelParams.map(parseLabel).filter((x): x is LabelFilter => Boolean(x));

  const services = useSqlQuery("profile-services", servicesSql(from, to));
  const serviceNames = (services.data ?? []).map((row) => String(row.s));
  const activeService = service && serviceNames.includes(service) ? service : (serviceNames[0] ?? service);

  const types = useSqlQuery(
    "profile-types",
    activeService ? typesSql(activeService, from, to) : undefined
  );
  const typeRows = (types.data ?? []).map((row) => ({ type: String(row.type), unit: String(row.unit ?? "") }));
  const activeType =
    typeRows.find((row) => row.type === type)?.type ??
    typeRows.find((row) => row.type === "cpu")?.type ??
    typeRows[0]?.type ??
    "";
  const unit = typeRows.find((row) => row.type === activeType)?.unit ?? "";

  const q: ProfileQuery | undefined =
    activeService && activeType
      ? { service: activeService, type: activeType, from, to, labels, traceId, spanId }
      : undefined;
  const span = to.getTime() - from.getTime();
  const baselineOffset = compare === "previous" ? span : compare === "1h" ? 3_600_000 : compare === "1d" ? 86_400_000 : 0;
  const qBase: ProfileQuery | undefined =
    q && compare !== "off"
      ? { ...q, from: new Date(from.getTime() - baselineOffset), to: new Date(to.getTime() - baselineOffset) }
      : undefined;

  const stacks = useSqlQuery("profile-stacks", q && stacksSql(q));
  const summary = useSqlQuery("profile-summary", q && summarySql(q));
  const baseStacks = useSqlQuery("profile-stacks-base", qBase && stacksSql(qBase));
  const baseSummary = useSqlQuery("profile-summary-base", qBase && summarySql(qBase));
  const bucket = bucketFor(from, to);
  const timeline = useSqlQuery("profile-timeline", q && timelineSql(q, bucket.sql));
  const labelOptions = useSqlQuery("profile-labels", q && labelsSql(q));

  const snapshot = isSnapshot(activeType);
  const snaps = Math.max(1, num(summary.data?.[0]?.snapshots));
  const baseSnaps = Math.max(1, num(baseSummary.data?.[0]?.snapshots));
  // Snapshot types (in-use memory, goroutines) are averaged over the
  // collections in the window rather than summed.
  const scale = snapshot ? 1 / snaps : 1;
  const baseScale = snapshot ? 1 / baseSnaps : 1;

  const current = React.useMemo(() => toStacks(stacks.data), [stacks.data]);
  const baseline = React.useMemo(() => toStacks(baseStacks.data), [baseStacks.data]);
  const diff = compare !== "off";
  const tree = React.useMemo(
    () =>
      diff
        ? buildDiffTree(
            baseline.map((s) => ({ ...s, value: s.value * baseScale })),
            current.map((s) => ({ ...s, value: s.value * scale }))
          )
        : buildTree(current, scale),
    [diff, baseline, current, scale, baseScale]
  );
  const total = current.reduce((sum, s) => sum + s.value, 0) * scale;
  const baseTotal = baseline.reduce((sum, s) => sum + s.value, 0) * baseScale;

  const functions = React.useMemo(() => {
    const a = topFunctions(current, scale);
    const b = diff ? topFunctions(baseline, baseScale) : undefined;
    const names = new Set([...a.keys(), ...(b?.keys() ?? [])]);
    return [...names]
      .map((name) => ({
        name,
        self: a.get(name)?.self ?? 0,
        total: a.get(name)?.total ?? 0,
        baseSelf: b?.get(name)?.self ?? 0,
      }))
      .sort((x, y) => y.self - x.self || y.total - x.total)
      .slice(0, 100);
  }, [current, baseline, diff, scale, baseScale]);

  const timelineData = (timeline.data ?? []).map((row) => ({
    time: new Date(num(row.t) / 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
    value: num(row.v),
  }));
  const timelineConfig = {
    value: { label: TYPE_LABELS[activeType] ?? activeType, colors: { light: ["#f97316"], dark: ["#fb923c"] } },
  } satisfies ChartConfig;

  const addLabel = (label: LabelFilter) => {
    const key = `${label.key}=${label.value}`;
    if (!labelParams.includes(key)) void setLabelParams([...labelParams, key]);
  };
  const removeLabel = (key: string) =>
    void setLabelParams(labelParams.filter((x) => x !== key).length ? labelParams.filter((x) => x !== key) : null);

  if (services.isSuccess && serviceNames.length === 0 && !custom) return <ProfilesEmpty />;

  const resetKey = `${activeService}|${activeType}|${labelParams.join(",")}|${traceId}|${spanId}|${compare}`;
  const loading = stacks.isLoading || types.isLoading || services.isLoading;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 p-4 lg:p-6" data-page="profiles">
      <div className="flex flex-wrap items-center gap-2">
        <Select value={activeService} onValueChange={(v) => void setService(v)}>
          <SelectTrigger className="w-48" size="sm" aria-label="Service" data-action="profile-service">
            <SelectValue placeholder="Service" />
          </SelectTrigger>
          <SelectContent className="rounded-xl">
            {serviceNames.map((name) => (
              <SelectItem key={name} value={name} className="rounded-lg">
                {name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={activeType} onValueChange={(v) => void setType(v)}>
          <SelectTrigger className="w-52" size="sm" aria-label="Profile type" data-action="profile-type">
            <SelectValue placeholder="Profile type" />
          </SelectTrigger>
          <SelectContent className="rounded-xl">
            {typeRows.map((row) => (
              <SelectItem key={row.type} value={row.type} className="rounded-lg">
                {TYPE_LABELS[row.type] ?? row.type}
                <span className="text-muted-foreground ml-1 text-xs">{row.type}</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <LabelPicker
          options={(labelOptions.data ?? []).map((row) => ({
            key: String(row.key),
            value: String(row.val),
            total: num(row.total),
          }))}
          unit={unit}
          onPick={addLabel}
        />
        <Select value={compare} onValueChange={(v) => void setCompare(v === "off" ? null : (v as Compare))}>
          <SelectTrigger className="w-48" size="sm" aria-label="Compare" data-action="profile-compare">
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="rounded-xl">
            {COMPARE.map((c) => (
              <SelectItem key={c} value={c} className="rounded-lg">
                {COMPARE_LABELS[c]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input
          className="h-8 w-56"
          placeholder="Highlight function…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          aria-label="Highlight function"
          data-action="profile-search"
        />
      </div>
      {(labels.length > 0 || traceId || custom) && (
        <div className="flex flex-wrap items-center gap-2" data-filters>
          {labelParams.map((key) => (
            <Badge key={key} variant="secondary" className="gap-1 font-mono" data-label-filter={key}>
              {key}
              <button type="button" aria-label={`Remove ${key}`} onClick={() => removeLabel(key)}>
                <IconX className="size-3" />
              </button>
            </Badge>
          ))}
          {traceId && (
            <Badge variant="outline" className="gap-1 font-mono" data-trace-filter={traceId}>
              trace {traceId.slice(0, 12)}…{spanId ? ` · span ${spanId}` : ""}
              <a className="underline" href={`/dashboard/traces/?trace=${traceId}`}>
                open trace
              </a>
              <button
                type="button"
                aria-label="Clear trace filter"
                onClick={() => {
                  void setTraceId(null);
                  void setSpanId(null);
                }}
              >
                <IconX className="size-3" />
              </button>
            </Badge>
          )}
          {custom && (
            <Badge variant="outline" className="gap-1" data-custom-range>
              {from.toLocaleString()} – {to.toLocaleTimeString()}
              <button
                type="button"
                aria-label="Use the header's time range"
                onClick={() => {
                  void setFromMs(null);
                  void setToMs(null);
                }}
              >
                <IconX className="size-3" />
              </button>
            </Badge>
          )}
        </div>
      )}

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <Stat
          title={snapshot ? "Average per collection" : "Total"}
          value={formatValue(total, unit)}
          note={diff ? `baseline ${formatValue(baseTotal, unit)} (${baseTotal > 0 ? `${total >= baseTotal ? "+" : ""}${(((total - baseTotal) / baseTotal) * 100).toFixed(1)}%` : "–"})` : TYPE_LABELS[activeType] ?? activeType}
          name="total"
        />
        <Stat title="Distinct stacks" value={String(current.length)} name="stacks" />
        <Stat title="Collections" value={String(num(summary.data?.[0]?.snapshots))} name="collections" />
        <Stat
          title="Samples linked to spans"
          value={percent(num(summary.data?.[0]?.linked), num(summary.data?.[0]?.rows))}
          name="linked"
        />
      </div>

      <Card className="py-4">
        <CardContent className="h-36 px-2 sm:px-6" data-chart-name="profile-timeline">
          <EvilBarChart data={timelineData} config={timelineConfig} className="h-full w-full" isLoading={timeline.isLoading}>
            <EvilBarChart.Grid />
            <EvilBarChart.XAxis dataKey="time" />
            <EvilBarChart.Tooltip />
            <EvilBarChart.Bar dataKey="value" />
          </EvilBarChart>
        </CardContent>
      </Card>

      <Tabs value={view} onValueChange={(v) => void setView(v === "both" ? null : (v as (typeof VIEWS)[number]))}>
        <TabsList>
          <TabsTrigger value="both">Table + flame graph</TabsTrigger>
          <TabsTrigger value="flamegraph">Flame graph</TabsTrigger>
          <TabsTrigger value="table">Top functions</TabsTrigger>
        </TabsList>
        {VIEWS.map((v) => (
          <TabsContent key={v} value={v}>
            {loading ? (
              <Skeleton className="h-96 w-full" />
            ) : current.length === 0 && baseline.length === 0 ? (
              <p className="text-muted-foreground p-6 text-sm" data-state="no-samples">
                No samples match these filters in this window.
              </p>
            ) : (
              <div className={v === "both" ? "grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]" : "grid gap-4"}>
                {v !== "flamegraph" && (
                  <FunctionsTable
                    rows={functions}
                    total={total}
                    unit={unit}
                    diff={diff}
                    onPick={(name) => setSearch(name)}
                    compact={v === "both"}
                  />
                )}
                {v !== "table" && (
                  <Card className="min-w-0 py-3">
                    <CardHeader className="px-4">
                      <CardTitle className="text-sm">
                        {diff ? "Diff flame graph" : "Flame graph"}
                      </CardTitle>
                      <CardDescription>
                        {diff
                          ? "Width is the larger of the two windows; red grew, blue shrank (compared with the baseline scaled to the same total). Click to zoom."
                          : "Click a frame to zoom; hover for its share."}
                      </CardDescription>
                    </CardHeader>
                    <CardContent className="px-2">
                      <Flamegraph tree={tree} unit={unit} diff={diff} search={search} resetKey={resetKey} />
                    </CardContent>
                  </Card>
                )}
              </div>
            )}
          </TabsContent>
        ))}
      </Tabs>
    </div>
  );
}

function Stat({ title, value, note, name }: { title: string; value: string; note?: string; name: string }) {
  return (
    <Card className="gap-1 py-4" data-stat={name}>
      <CardHeader className="px-4">
        <CardDescription>{title}</CardDescription>
        <CardTitle className="text-xl tabular-nums">{value}</CardTitle>
        {note && <p className="text-muted-foreground text-xs">{note}</p>}
      </CardHeader>
    </Card>
  );
}

function LabelPicker({
  options,
  unit,
  onPick,
}: {
  options: { key: string; value: string; total: number }[];
  unit: string;
  onPick: (label: LabelFilter) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const byKey = new Map<string, typeof options>();
  for (const option of options) byKey.set(option.key, [...(byKey.get(option.key) ?? []), option]);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm" data-action="profile-label-filter">
          Filter by label
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-80 p-0" align="start">
        <Command>
          <CommandInput placeholder="Label or value…" />
          <CommandList>
            <CommandEmpty>No string labels on these samples.</CommandEmpty>
            {[...byKey.entries()].map(([key, values]) => (
              <CommandGroup key={key} heading={key}>
                {values.map((option) => (
                  <CommandItem
                    key={`${key}=${option.value}`}
                    value={`${key}=${option.value}`}
                    data-label-option={`${key}=${option.value}`}
                    onSelect={() => {
                      onPick({ key, value: option.value });
                      setOpen(false);
                    }}
                  >
                    <span className="truncate font-mono text-xs">{option.value}</span>
                    <span className="text-muted-foreground ml-auto text-xs tabular-nums">
                      {formatValue(option.total, unit)}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function FunctionsTable({
  rows,
  total,
  unit,
  diff,
  onPick,
  compact,
}: {
  rows: { name: string; self: number; total: number; baseSelf: number }[];
  total: number;
  unit: string;
  diff: boolean;
  onPick: (name: string) => void;
  compact: boolean;
}) {
  return (
    <Card className="min-w-0 py-3">
      <CardHeader className="px-4">
        <CardTitle className="text-sm">Top functions</CardTitle>
        <CardDescription>Self is time or bytes in the function itself; total includes its callees. Click to highlight.</CardDescription>
      </CardHeader>
      <CardContent className={`px-0 ${compact ? "max-h-[640px] overflow-y-auto" : ""}`}>
        <Table data-table="top-functions" data-rows={rows.length} className="table-fixed">
          <TableHeader>
            <TableRow>
              <TableHead className="pl-4">Function</TableHead>
              <TableHead className="w-24 text-right">Self</TableHead>
              {!compact && <TableHead className="w-20 text-right">Self %</TableHead>}
              <TableHead className={`w-24 text-right ${diff ? "" : "pr-4"}`}>Total</TableHead>
              {diff && <TableHead className="w-28 pr-4 text-right">Self change</TableHead>}
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row) => {
              const change = row.self - row.baseSelf;
              return (
                <TableRow key={row.name} className="cursor-pointer" onClick={() => onPick(row.name)} data-function={row.name}>
                  <TableCell className="truncate pl-4 font-mono text-xs" title={row.name}>
                    {row.name}
                  </TableCell>
                  <TableCell className="text-right text-xs tabular-nums">{formatValue(row.self, unit)}</TableCell>
                  {!compact && (
                    <TableCell className="text-right text-xs tabular-nums">{percent(row.self, total)}</TableCell>
                  )}
                  <TableCell className={`text-right text-xs tabular-nums ${diff ? "" : "pr-4"}`}>{formatValue(row.total, unit)}</TableCell>
                  {diff && (
                    <TableCell
                      className={`pr-4 text-right text-xs tabular-nums ${change > 0 ? "text-destructive" : change < 0 ? "text-sky-600 dark:text-sky-400" : ""}`}
                    >
                      {change > 0 ? "+" : ""}
                      {formatValue(change, unit)}
                    </TableCell>
                  )}
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}

function ProfilesEmpty() {
  return (
    <div className="p-4 lg:p-6">
      <Empty className="border" data-state="profiles-setup">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <IconFlame />
          </EmptyMedia>
          <EmptyTitle>No profiles in this window</EmptyTitle>
          <EmptyDescription>
            Send OTLP profiles to <code>/v1development/profiles</code>, or point a Pyroscope SDK at
            sigiro. For Go, one call starts continuous CPU and heap profiling:
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent className="max-w-2xl text-left">
          <pre className="bg-muted w-full overflow-x-auto rounded-md p-3 text-xs">
            {`p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "http://localhost:4318",
})
defer p.Stop()`}
          </pre>
        </EmptyContent>
      </Empty>
    </div>
  );
}
