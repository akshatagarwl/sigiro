"use client";

import { toUrlString, type DateTimeRangeValue } from "@loykin/datetime-range";
import { useQuery } from "@tanstack/react-query";
import {
  parseAsBoolean,
  parseAsInteger,
  parseAsString,
  parseAsStringLiteral,
  useQueryState,
  useQueryStates,
} from "nuqs";
import * as React from "react";

import { query, type Row } from "./query";
import { resolveTimeWindow } from "./time-window";

/**
 * The header's time range, service and live switch, shared by every page
 * through the URL (`?range=6h&service=api&live=true`) so a link reproduces the
 * view.
 */
export const RANGES = {
  "15m": { label: "Last 15 minutes", ms: 900_000 },
  "1h": { label: "Last hour", ms: 3_600_000 },
  "6h": { label: "Last 6 hours", ms: 21_600_000 },
  "24h": { label: "Last 24 hours", ms: 86_400_000 },
  "7d": { label: "Last 7 days", ms: 604_800_000 },
} as const;
export type RangeKey = keyof typeof RANGES;
const RANGE_KEYS = Object.keys(RANGES) as RangeKey[];

const LIVE_EVERY_MS = 10_000;

type Controls = {
  range: RangeKey;
  setRange: (range: RangeKey) => void;
  hasCustomWindow: boolean;
  start: DateTimeRangeValue;
  end: DateTimeRangeValue;
  setWindow: (start: DateTimeRangeValue, end: DateTimeRangeValue) => void;
  windowKey: string;
  windowError: string | null;
  /** One service, or "" for all. */
  service: string;
  setService: (service: string) => void;
  live: boolean;
  setLive: (live: boolean) => void;
  /** Bumps on every refresh and every live tick; the window ends at the bump. */
  tick: number;
  refresh: () => void;
  from: Date;
  to: Date;
};

const ControlsContext = React.createContext<Controls | null>(null);

export function ControlsProvider({ children }: { children: React.ReactNode }) {
  const [
    {
      range,
      timestamp,
      from: customFrom,
      to: customTo,
      timeStart,
      timeEnd,
      live,
    },
    setTime,
  ] = useQueryStates({
    range: parseAsStringLiteral(RANGE_KEYS).withDefault("24h"),
    timestamp: parseAsString,
    from: parseAsInteger,
    to: parseAsInteger,
    timeStart: parseAsString,
    timeEnd: parseAsString,
    live: parseAsBoolean.withDefault(false),
    cursor: parseAsString,
    after: parseAsString,
    direction: parseAsString,
  });
  const hasCustomWindow =
    timestamp !== null ||
    customFrom !== null ||
    customTo !== null ||
    timeStart !== null ||
    timeEnd !== null;
  const [service, setService] = useQueryState(
    "service",
    parseAsString.withDefault("")
  );
  const [tick, setTick] = React.useState(0);
  const [now, setNow] = React.useState(() => Date.now());
  const refresh = React.useCallback(() => {
    setNow(Date.now());
    setTick((value) => value + 1);
  }, []);

  React.useEffect(() => {
    if (!live) return;
    const timer = setInterval(refresh, LIVE_EVERY_MS);
    return () => clearInterval(timer);
  }, [live, refresh]);

  React.useEffect(refresh, [range, timeStart, timeEnd, refresh]);

  // Honor existing profile links and table chart selections as well as the
  // header's new relative/absolute endpoints; every page sees the same window.
  const legacyDates = timestamp?.match(/^(\d+)(?:-(\d+))?$/);
  const invalidLegacyTimestamp = timestamp !== null && !legacyDates;
  const legacyStart =
    customFrom ?? (legacyDates ? Number(legacyDates[1]) : null);
  const legacyEnd =
    customTo ??
    (legacyDates
      ? Number(legacyDates[2] ?? Number(legacyDates[1]) + 86_400_000)
      : null);
  const startText =
    timeStart ?? (legacyStart !== null ? String(legacyStart) : `now-${range}`);
  const endText = timeEnd ?? (legacyEnd !== null ? String(legacyEnd) : "now");
  const windowKey = JSON.stringify([
    startText,
    endText,
    customFrom,
    customTo,
    timestamp,
  ]);
  const resolved = React.useMemo(() => {
    try {
      if ((timeStart === null) !== (timeEnd === null))
        throw new Error("Provide both time endpoints");
      if (timeStart === null && (customFrom === null) !== (customTo === null))
        throw new Error("Provide both time endpoints");
      if (timeStart === null && invalidLegacyTimestamp)
        throw new Error("Invalid timestamp selection");
      return { ...resolveTimeWindow(startText, endText, now), error: null };
    } catch (error) {
      return {
        ...resolveTimeWindow(`now-${range}`, "now", now),
        error: error instanceof Error ? error.message : "Invalid time range",
      };
    }
  }, [
    startText,
    endText,
    timeStart,
    timeEnd,
    customFrom,
    customTo,
    timestamp,
    invalidLegacyTimestamp,
    range,
    now,
  ]);

  const value = React.useMemo<Controls>(
    () => ({
      range,
      // A deliberate relative-range choice replaces view-local time overrides
      // in one URL transaction; changing range must change the actual SQL window.
      setRange: (next) =>
        void setTime({
          range: next,
          timestamp: null,
          from: null,
          to: null,
          timeStart: null,
          timeEnd: null,
          cursor: null,
          after: null,
          direction: null,
        }),
      hasCustomWindow,
      start: resolved.start,
      end: resolved.end,
      windowKey,
      windowError: resolved.error,
      setWindow: (start, end) => {
        const nextStart = toUrlString(start);
        const nextEnd = toUrlString(end);
        resolveTimeWindow(nextStart, nextEnd, Date.now());
        const preset = RANGE_KEYS.find(
          (key) => nextStart === `now-${key}` && nextEnd === "now"
        );
        void setTime({
          range: preset ?? range,
          timeStart: preset ? null : nextStart,
          timeEnd: preset ? null : nextEnd,
          timestamp: null,
          from: null,
          to: null,
          cursor: null,
          after: null,
          direction: null,
          live: nextEnd === "now" ? live : null,
        });
        refresh();
      },
      service,
      setService: (next) => void setService(next || null),
      live,
      setLive: (next) => void setTime({ live: next || null }),
      tick,
      refresh,
      from: resolved.from,
      to: resolved.to,
    }),
    [
      range,
      setTime,
      hasCustomWindow,
      service,
      setService,
      live,
      resolved,
      windowKey,
      tick,
      refresh,
      now,
    ]
  );

  return (
    <ControlsContext.Provider value={value}>
      {children}
    </ControlsContext.Provider>
  );
}

export function useControls(): Controls {
  const value = React.useContext(ControlsContext);
  if (!value) throw new Error("useControls outside ControlsProvider");
  return value;
}

export type Window = { from: Date; to: Date; service: string };

/**
 * One panel's SQL over the header's window and service, re-run on refresh and
 * live ticks (each moves the window, so the SQL text is the cache key).
 */
export function useSql(name: string, sql: (window: Window) => string) {
  const { from, to, service, windowKey, windowError, tick } = useControls();
  const text = sql({ from, to, service });
  const scope = JSON.stringify([windowKey, service]);
  return useQuery<Row[]>({
    queryKey: [name, text, scope, tick],
    queryFn: ({ signal }) => query(text, signal),
    enabled: windowError === null,
    // Refresh can retain the last snapshot with an Updating state. A different
    // service or range must not inherit data labeled with the new scope.
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[2] === scope ? previous : undefined,
    retry: false,
  });
}
