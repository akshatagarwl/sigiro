'use client';

import { CartesianGrid, Line, LineChart, ReferenceLine, XAxis, YAxis } from 'recharts';
import { DEPLOY_INDEX, SERIES, SHIFT, SHIFT_INDEX } from '@/lib/changepoint';
import { type ChartConfig, ChartContainer } from '@/components/ui/chart';

const config = {
  errorPct: { label: 'error rate %', color: 'var(--chart-1)' },
  p95Ms: { label: 'p95 ms', color: 'var(--chart-1)' },
} satisfies ChartConfig;

const data = SERIES.map((b, i) => ({
  ...b,
  i,
  // p95 is stored in µs; charting it in ms just keeps the axis readable.
  p95Ms: Math.round(b.p95Us / 1000),
}));

/**
 * Two series, one shift, no threshold anywhere. `step` interpolation is the
 * honest curve for bucketed data — each point is a 5-minute mean, not a sample
 * on a continuous line. The dashed rules are the before/after means the anomaly
 * row actually stores, and the deploy marker is the event correlation picks up.
 */
function Series({
  dataKey,
  before,
  after,
  format,
}: {
  dataKey: 'errorPct' | 'p95Ms';
  before: number;
  after: number;
  format: (v: number) => string;
}) {
  return (
    <ChartContainer config={config} className="aspect-[3/1] w-full">
      <LineChart data={data} margin={{ top: 8, right: 8, bottom: 0, left: 0 }}>
        <CartesianGrid vertical={false} strokeDasharray="2 4" />
        <XAxis dataKey="t" tickLine={false} axisLine={false} interval={3} tickMargin={8} />
        <YAxis
          tickLine={false}
          axisLine={false}
          width={44}
          tickFormatter={(v: number) => format(v)}
        />
        <ReferenceLine
          y={before}
          stroke="var(--muted-foreground)"
          strokeDasharray="3 3"
          label={{ value: format(before), position: 'insideTopLeft', fontSize: 11 }}
        />
        <ReferenceLine
          y={after}
          stroke="var(--muted-foreground)"
          strokeDasharray="3 3"
          label={{ value: format(after), position: 'insideTopLeft', fontSize: 11 }}
        />
        <ReferenceLine
          x={SERIES[DEPLOY_INDEX].t}
          stroke="var(--muted-foreground)"
          label={{ value: 'deploy 9f2c1ab', position: 'insideTopRight', fontSize: 11 }}
        />
        <ReferenceLine x={SERIES[SHIFT_INDEX].t} stroke="var(--chart-1)" strokeWidth={2} />
        <Line
          dataKey={dataKey}
          type="step"
          stroke="var(--chart-1)"
          strokeWidth={2}
          dot={false}
          isAnimationActive={false}
        />
      </LineChart>
    </ChartContainer>
  );
}

export function Changepoint() {
  return (
    <div className="flex flex-col gap-6">
      <figure>
        <figcaption className="mb-1 font-mono text-xs text-muted-foreground">
          error_rate — {SHIFT.errorPctBefore}% → {SHIFT.errorPctAfter}% at {SHIFT.at}
        </figcaption>
        <Series
          dataKey="errorPct"
          before={SHIFT.errorPctBefore}
          after={SHIFT.errorPctAfter}
          format={(v) => `${v}%`}
        />
      </figure>
      <figure>
        <figcaption className="mb-1 font-mono text-xs text-muted-foreground">
          latency_p95 — {Math.round(SHIFT.p95UsBefore / 1000)}ms →{' '}
          {(SHIFT.p95UsAfter / 1_000_000).toFixed(2)}s at {SHIFT.at}, same bucket, same incident
        </figcaption>
        <Series
          dataKey="p95Ms"
          before={Math.round(SHIFT.p95UsBefore / 1000)}
          after={Math.round(SHIFT.p95UsAfter / 1000)}
          format={(v) => `${v}ms`}
        />
      </figure>
    </div>
  );
}
