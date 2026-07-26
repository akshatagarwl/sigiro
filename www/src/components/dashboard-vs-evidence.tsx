'use client';

import {
  Comparison,
  ComparisonHandle,
  ComparisonItem,
} from '@/components/kibo-ui/comparison';
import { NumberTicker } from '@/components/ui/number-ticker';
import { SERIES, SHIFT } from '@/lib/changepoint';

/** The same incident, drawn the way a chart shows it and the way an agent reads it. */
export function DashboardVsEvidence() {
  const max = Math.max(...SERIES.map((b) => b.errorPct));

  return (
    <div className="flex flex-col gap-3">
      <Comparison className="h-72 rounded-xl border border-border">
        <ComparisonItem position="left" className="bg-background p-5">
          <p className="font-mono text-xs text-muted-foreground">what a chart shows an agent</p>
          <div className="mt-4 flex h-40 items-end gap-1" aria-hidden>
            {SERIES.map((b) => (
              <span
                key={b.t}
                className="flex-1 bg-muted-foreground/40"
                style={{ height: `${Math.max(2, (b.errorPct / max) * 100)}%` }}
              />
            ))}
          </div>
          <p className="mt-3 text-sm text-muted-foreground">
            Pixels. Something went up around {SHIFT.at}. Which operation, which deploy, and what to
            do next are all left as an exercise.
          </p>
        </ComparisonItem>

        <ComparisonItem position="right" className="bg-background p-5">
          <p className="font-mono text-xs text-muted-foreground">what sigiro returns</p>
          <pre className="mt-4 overflow-hidden text-[11px] leading-relaxed">
            <code className="font-mono">{`"findings": [{
  "severity": "critical",
  "kind": "incident",
  "summary": "2 correlated shifts at ${SHIFT.at}",
  "incident": {
    "deploy": "9f2c1ab at ${SHIFT.deployAt}" }}],
"error_breakdown": [
  { "error_type": "timeout", "count": 246 }],
"drill_down_sql": "SELECT * FROM sigiro_spans ..."`}</code>
          </pre>
        </ComparisonItem>
        <ComparisonHandle />
      </Comparison>

      <p className="text-sm text-muted-foreground">
        Drag the handle. Same incident, same window. One side needs a person to read it; the other
        names the deploy, says the operation failed{' '}
        <span className="font-mono text-foreground">
          <NumberTicker value={246} className="tabular-nums" />
        </span>{' '}
        times by timeout rather than by 500, and hands over the query that proves it.
      </p>
    </div>
  );
}
