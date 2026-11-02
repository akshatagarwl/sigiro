"use client";

import { EChartsAreaChart, type ChartConfig } from "@/components/evilcharts/charts/echarts-area-chart";
import { cn } from "@/lib/utils";
import { useState } from "react";

import { TOKENS } from "@/lib/sigiro/palette";

// Series colours follow the app theme (see lib/sigiro/palette.ts) instead of
// the block's sample palette; the data comes from the caller.
const SERIES = [
  { key: "p99", label: "P99", color: TOKENS.error },
  { key: "p95", label: "P95", color: TOKENS.warning },
  { key: "p50", label: "P50", color: TOKENS.info },
] as const;

export type LatencyPoint = { time: string; p50: number | null; p95: number | null; p99: number | null };

const chartConfig = {
  p99: { label: "P99", colors: { light: [TOKENS.error], dark: [TOKENS.error] } },
  p95: { label: "P95", colors: { light: [TOKENS.warning], dark: [TOKENS.warning] } },
  p50: { label: "P50", colors: { light: [TOKENS.info], dark: [TOKENS.info] } },
} satisfies ChartConfig;

/** Milliseconds, or seconds once a value reaches a second. */
const formatLatency = (ms: number | null): [string, string] =>
  ms === null ? ["–", ""] : ms >= 1000 ? [(ms / 1000).toFixed(2), "s"] : [String(ms), "ms"];

// A window with few measured buckets (one burst of traffic) would draw an
// almost empty chart: a lone point has no line to draw, and ECharts drops
// symbols on a crowded category axis. So a sparse window plots only its
// measured buckets, each with a dot, joined across the gaps.
const SPARSE_POINTS = 12;

export function EChartsLatencyAreaChart({
  data: chartData,
  latest,
  isLoading,
}: {
  data: LatencyPoint[];
  latest: Record<"p50" | "p95" | "p99", number | null>;
  isLoading?: boolean;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const measured = chartData.filter((point) => point.p95 !== null).length;
  const sparse = measured > 0 && measured <= SPARSE_POINTS;
  const plotted = sparse ? chartData.filter((point) => point.p95 !== null) : chartData;

  return (
    <div className="flex h-full w-full flex-col p-4">
      <div className="grid grid-cols-2 gap-y-2 sm:grid-cols-3 sm:gap-y-4">
        {SERIES.map(({ key, label, color }) => (
          <button
            key={key}
            type="button"
            onClick={() => setSelected((prev) => (prev === key ? null : key))}
            className={cn(
              "border-border flex cursor-pointer flex-row items-center gap-1.5 px-3 text-left transition-opacity sm:flex-col sm:items-start sm:gap-1.5 sm:px-4 sm:first:pl-1 sm:[&:not(:first-child)]:border-l [&:nth-child(even)]:border-l",
              selected !== null && selected !== key && "opacity-40",
            )}
          >
            <div className="text-primary flex items-center gap-1.5 text-xs font-medium sm:gap-2">
              <span className="size-2 shrink-0 rounded-[2px]" style={{ backgroundColor: color }} />
              {label}
            </div>
            <span className="text-muted-foreground text-xs sm:hidden">–</span>
            <div className="leading-none">
              <span className="text-primary text-sm font-medium tracking-tight sm:text-xl">
                {formatLatency(latest[key])[0]}
              </span>
              <span className="text-muted-foreground ml-0.5 text-xs font-light sm:ml-1 sm:text-sm">
                {formatLatency(latest[key])[1]}
              </span>
            </div>
          </button>
        ))}
      </div>
      <EChartsAreaChart
        data={plotted}
        config={chartConfig}
        xDataKey="time"
        className="mt-4 min-h-0 w-full flex-1"
        curveType="linear"
        enableHoverHighlight
        selectedDataKey={selected}
        onSelectionChange={setSelected}
        isLoading={isLoading}
      >
        <EChartsAreaChart.Grid />
        <EChartsAreaChart.XAxis
          dataKey="time"
                    />
        <EChartsAreaChart.YAxis />
        <EChartsAreaChart.Tooltip />
        <EChartsAreaChart.Area dataKey="p50" variant="gradient" strokeVariant="solid" isClickable connectNulls>
          {sparse && <EChartsAreaChart.Dot />}
        </EChartsAreaChart.Area>
        <EChartsAreaChart.Area dataKey="p95" variant="gradient" strokeVariant="solid" isClickable connectNulls>
          {sparse && <EChartsAreaChart.Dot />}
        </EChartsAreaChart.Area>
        <EChartsAreaChart.Area dataKey="p99" variant="gradient" strokeVariant="solid" isClickable connectNulls>
          {sparse && <EChartsAreaChart.Dot />}
        </EChartsAreaChart.Area>
      </EChartsAreaChart>
    </div>
  );
}
