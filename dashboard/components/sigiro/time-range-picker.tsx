"use client";

import { Popover } from "@base-ui/react/popover";
import {
  DatetimeRange,
  relativeAgo,
  relativeNow,
  toUrlString,
  type DatetimeRangeUIAdapter,
  type DateTimeRangeValue,
  type QuickPreset,
} from "@loykin/datetime-range";
import { CalendarClock } from "lucide-react";
import * as React from "react";

import { resolveTimeWindow } from "@/lib/sigiro/time-window";

import "@loykin/datetime-range/styles";

import "./time-range-picker.css";

// Module-scoped: changing adapter identity remounts the upstream draft editors.
const adapter: DatetimeRangeUIAdapter = {
  PopoverContent: function RangePopup({
    container,
    align = "start",
    alignOffset,
    side = "bottom",
    sideOffset = 8,
    disableAnchorTracking,
    collisionAvoidance,
    className,
    children,
    ...props
  }) {
    return (
      <Popover.Portal container={container}>
        <Popover.Positioner
          className="sigiro-time-range-positioner"
          align={align}
          alignOffset={alignOffset}
          side={side}
          sideOffset={sideOffset}
          collisionPadding={8}
          disableAnchorTracking={disableAnchorTracking}
          collisionAvoidance={collisionAvoidance}
        >
          <Popover.Popup
            {...props}
            aria-label="Choose time range"
            className={`sigiro-time-range-popup ${className ?? ""}`}
          >
            <div className="text-muted-foreground border-b px-3 py-2 text-xs">
              Date/time fields use browser time ·{" "}
              {Intl.DateTimeFormat().resolvedOptions().timeZone}
            </div>
            {children}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    );
  },
};

const presets: QuickPreset[] = [
  ["Last 5 minutes", 5, "Minutes ago"],
  ["Last 15 minutes", 15, "Minutes ago"],
  ["Last 30 minutes", 30, "Minutes ago"],
  ["Last hour", 1, "Hours ago"],
  ["Last 6 hours", 6, "Hours ago"],
  ["Last 24 hours", 24, "Hours ago"],
  ["Last 7 days", 7, "Days ago"],
].map(([label, amount, unit]) => ({
  label: String(label),
  start: relativeAgo(
    Number(amount),
    unit as "Minutes ago" | "Hours ago" | "Days ago"
  ),
  end: relativeNow(),
}));

function rangeLabel(start: DateTimeRangeValue, end: DateTimeRangeValue) {
  if (end.type === "relative" && end.relativeNow) {
    const preset = presets.find(
      ({ start: candidate }) =>
        start.type === "relative" &&
        !start.relativeNow &&
        Number(start.relativeValue) === Number(candidate.relativeValue) &&
        start.relativeFormat === candidate.relativeFormat
    );
    if (preset) return preset.label;
  }
  const label = (value: DateTimeRangeValue) => {
    if (value.type === "absolute")
      return (
        value.absoluteValue
          ?.toISOString()
          .replace("T", " ")
          .replace(".000Z", "Z") ?? "Unset"
      );
    if (value.relativeNow) return "now";
    return `${value.relativeValue} ${value.relativeFormat ?? "ago"}`;
  };
  return `${label(start)} → ${label(end)}`;
}

/** Controlled adapter only: live refresh and query windows remain the caller's job. */
export function TimeRangePicker({
  start,
  end,
  onChange,
}: {
  start: DateTimeRangeValue;
  end: DateTimeRangeValue;
  onChange: (start: DateTimeRangeValue, end: DateTimeRangeValue) => void;
}) {
  const [error, setError] = React.useState<string | null>(null);
  return (
    <div className="max-w-full min-w-0">
      <DatetimeRange
        startTime={start}
        endTime={end}
        onChange={(from, to) => {
          try {
            resolveTimeWindow(toUrlString(from), toUrlString(to), Date.now());
            onChange(from, to);
            setError(null);
          } catch (cause) {
            setError(
              cause instanceof Error ? cause.message : "Invalid time range"
            );
          }
        }}
        quickPresets={presets}
        calendarMode="inline"
        precision="second"
        labels={{
          start: "From",
          end: "To",
          quickRanges: "Quick ranges",
          apply: "Apply range",
        }}
        uiAdapter={adapter}
        // Use browser-local time consistently. Upstream's segmented text input
        // ignores timezone, although its calendar honors it: don't expose a
        // misleading zone override until both editors agree.
        renderTrigger={(props) => (
          <button
            {...props}
            type="button"
            className="sigiro-time-range-trigger"
            title="Edit dates in browser-local time; absolute toolbar timestamps are UTC (Z)"
            aria-label={`Time range: ${rangeLabel(start, end)}`}
          >
            <CalendarClock aria-hidden="true" size={15} />
            <span>{rangeLabel(start, end)}</span>
          </button>
        )}
      />
      {error && (
        <p role="alert" className="text-destructive mt-1 text-xs">
          {error}. Choose a valid range.
        </p>
      )}
    </div>
  );
}
