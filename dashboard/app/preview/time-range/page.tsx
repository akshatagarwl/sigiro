"use client";

import * as React from "react";

import { TimeRangePicker } from "@/components/sigiro/time-range-picker";
import { Button } from "@/components/ui/button";
import {
  parseTimeEndpoint,
  toUrlString,
  type DateTimeRangeValue,
} from "@/lib/sigiro/time-window";

/** Isolated evaluation: deliberately does not change shared query controls. */
export default function TimeRangePreview() {
  const [start, setStart] = React.useState<DateTimeRangeValue>(
    () => parseTimeEndpoint("now-15m", Date.now()).value
  );
  const [end, setEnd] = React.useState<DateTimeRangeValue>({
    type: "relative",
    relativeNow: true,
  });
  const [timezone, setTimezone] = React.useState("");
  React.useEffect(() => {
    setTimezone(Intl.DateTimeFormat().resolvedOptions().timeZone);
  }, []);
  const [snapshot, setSnapshot] = React.useState("");

  const resolve = (from: DateTimeRangeValue, to: DateTimeRangeValue) => {
    const now = Date.now();
    setSnapshot(
      `${parseTimeEndpoint(toUrlString(from), now).date.toISOString()} → ${parseTimeEndpoint(toUrlString(to), now).date.toISOString()}`
    );
  };

  return (
    <main className="mx-auto flex w-full max-w-4xl flex-col gap-6 p-6">
      <header className="space-y-2">
        <h1 className="text-xl font-semibold">Time-range picker preview</h1>
        <p className="text-muted-foreground text-sm">
          UI SDK. Local evaluation only: this picker does not run queries or
          replace the dashboard header.
        </p>
      </header>
      <section className="space-y-6 rounded-xl border p-6">
        <div className="flex flex-wrap items-center gap-4">
          <TimeRangePicker
            start={start}
            end={end}
            onChange={(from, to) => {
              setStart(from);
              setEnd(to);
              resolve(from, to);
            }}
          />
          <span className="text-muted-foreground text-xs">
            Browser time{timezone ? ` · ${timezone}` : ""}
          </span>
        </div>
        <div className="space-y-2 text-sm">
          <p className="text-muted-foreground">
            Serialized range (not applied to the URL)
          </p>
          <code className="block break-all">
            from={toUrlString(start)}&amp;to={toUrlString(end)}
          </code>
          <Button
            variant="outline"
            size="sm"
            onClick={() => resolve(start, end)}
          >
            Resolve at current time
          </Button>
          <output
            aria-live="polite"
            className="block font-mono text-xs break-all"
          >
            {snapshot ||
              "Apply a range to inspect its resolved UTC timestamps."}
          </output>
        </div>
      </section>
      <p className="text-muted-foreground text-sm">
        Evaluation: presets; relative versus absolute ranges; Apply/Cancel;
        keyboard focus and Escape; narrow screens; light/dark themes; time-zone
        consistency. Relative ranges should remain relative until query
        execution.
      </p>
    </main>
  );
}
