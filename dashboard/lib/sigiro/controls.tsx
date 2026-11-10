"use client";

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
  const [{ range, timestamp, from: customFrom, to: customTo }, setTime] =
    useQueryStates({
      range: parseAsStringLiteral(RANGE_KEYS).withDefault("24h"),
      timestamp: parseAsString,
      from: parseAsInteger,
      to: parseAsInteger,
    });
  const hasCustomWindow =
    timestamp !== null || customFrom !== null || customTo !== null;
  const [live, setLive] = useQueryState(
    "live",
    parseAsBoolean.withDefault(false)
  );
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

  React.useEffect(refresh, [range, refresh]);

  const value = React.useMemo<Controls>(
    () => ({
      range,
      // A deliberate relative-range choice replaces view-local time overrides
      // in one URL transaction; changing range must change the actual SQL window.
      setRange: (next) =>
        void setTime({ range: next, timestamp: null, from: null, to: null }),
      hasCustomWindow,
      service,
      setService: (next) => void setService(next || null),
      live,
      setLive: (next) => void setLive(next || null),
      tick,
      refresh,
      from: new Date(now - RANGES[range].ms),
      to: new Date(now),
    }),
    [
      range,
      setTime,
      hasCustomWindow,
      service,
      setService,
      live,
      setLive,
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
  const { from, to, service, range } = useControls();
  const text = sql({ from, to, service });
  const scope = JSON.stringify([range, service]);
  return useQuery<Row[]>({
    queryKey: [name, text, scope],
    queryFn: ({ signal }) => query(text, signal),
    // Refresh can retain the last snapshot with an Updating state. A different
    // service or range must not inherit data labeled with the new scope.
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[2] === scope ? previous : undefined,
    retry: false,
  });
}
