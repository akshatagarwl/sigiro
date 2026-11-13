import type { DateTimeRangeValue } from "@loykin/datetime-range";
import { subMonths, subYears } from "date-fns";

const UNITS = {
  s: "Seconds ago",
  m: "Minutes ago",
  h: "Hours ago",
  d: "Days ago",
  w: "Weeks ago",
  M: "Months ago",
  y: "Years ago",
} as const;
const MS = { s: 1000, m: 60_000, h: 3_600_000, d: 86_400_000, w: 604_800_000 };

/** Strict parsing: upstream silently turns malformed URL values into "now". */
export function parseTimeEndpoint(
  text: string,
  now: number
): {
  value: DateTimeRangeValue;
  date: Date;
} {
  if (text === "now")
    return {
      value: { type: "relative", relativeNow: true },
      date: new Date(now),
    };
  const relative = /^now-(\d+)([smhdwMy])$/.exec(text);
  if (relative) {
    const amount = Number(relative[1]);
    if (!Number.isSafeInteger(amount)) throw new Error("Invalid relative time");
    const unit = relative[2] as keyof typeof UNITS;
    const date =
      unit === "M"
        ? subMonths(now, amount)
        : unit === "y"
          ? subYears(now, amount)
          : new Date(now - amount * MS[unit]);
    if (!Number.isFinite(date.getTime()))
      throw new Error("Time is outside the supported date range");
    return {
      value: {
        type: "relative",
        relativeValue: String(amount),
        relativeFormat: UNITS[unit],
      },
      date,
    };
  }
  if (/^-?\d+$/.test(text)) {
    const ms = Number(text);
    const date = new Date(ms);
    if (Number.isSafeInteger(ms) && Number.isFinite(date.getTime())) {
      return { value: { type: "absolute", absoluteValue: date }, date };
    }
  }
  throw new Error("Invalid time endpoint");
}

/** Both endpoints resolve against one clock snapshot, including live refreshes. */
export function resolveTimeWindow(start: string, end: string, now: number) {
  const from = parseTimeEndpoint(start, now);
  const to = parseTimeEndpoint(end, now);
  if (from.date >= to.date) throw new Error("Start must be earlier than end");
  return { from: from.date, to: to.date, start: from.value, end: to.value };
}
