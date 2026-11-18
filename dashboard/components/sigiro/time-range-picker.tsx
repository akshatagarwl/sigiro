"use client";

import { toUrlString, type DateTimeRangeValue } from "@/lib/sigiro/time-window";
import { Popover, PopoverContent, PopoverTrigger, PopoverTitle, PopoverClose } from "@ui-sdk/react/popover";
import { Input } from "@ui-sdk/react/input";
import { Label } from "@ui-sdk/react/label";
import { Button } from "@/components/ui/button";
import { CalendarClock } from "lucide-react";
import * as React from "react";
import { parseTimeEndpoint, resolveTimeWindow } from "@/lib/sigiro/time-window";
import { format } from "date-fns";

const presets = [["Last 5 minutes", "now-5m"], ["Last 15 minutes", "now-15m"], ["Last 30 minutes", "now-30m"], ["Last hour", "now-1h"], ["Last 6 hours", "now-6h"], ["Last 24 hours", "now-24h"], ["Last 7 days", "now-7d"]] as const;
const display = (value: DateTimeRangeValue) => value.type === "absolute" ? value.absoluteValue?.toISOString() ?? "" : toUrlString(value);
const endpoint = (text: string) => {
  if (text.startsWith("now") || /^-?\d+$/.test(text)) return text;
  const value = Date.parse(text);
  if (!Number.isFinite(value)) throw new Error("Use now, now-15m, or an ISO timestamp");
  return String(value);
};

const localDate = (text: string) => {
  try { return format(parseTimeEndpoint(endpoint(text), Date.now()).date, "yyyy-MM-dd'T'HH:mm:ss"); }
  catch { return ""; }
};

export function TimeRangePicker({ start, end, onChange }: { start: DateTimeRangeValue; end: DateTimeRangeValue; onChange: (start: DateTimeRangeValue, end: DateTimeRangeValue) => void }) {
  const [open, setOpen] = React.useState(false);
  const [from, setFrom] = React.useState(display(start));
  const [to, setTo] = React.useState(display(end));
  const [error, setError] = React.useState("");
  const id = React.useId();
  const label = toUrlString(end) === "now" ? presets.find(([, value]) => value === toUrlString(start))?.[0] : undefined;
  const apply = (a = from, b = to) => {
    try {
      const window = resolveTimeWindow(endpoint(a.trim()), endpoint(b.trim()), Date.now());
      onChange(window.start, window.end);
      setError("");
      setOpen(false);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  };
  return <Popover open={open} onOpenChange={(next) => { if (next) { setFrom(display(start)); setTo(display(end)); setError(""); } setOpen(next); }}>
    <PopoverTrigger aria-label={`Time range: ${label ?? `${display(start)} → ${display(end)}`}`} className="max-w-full gap-2"><CalendarClock aria-hidden="true" size={15} /><span className="truncate">{label ?? `${display(start)} → ${display(end)}`}</span></PopoverTrigger>
    <PopoverContent align="end" sideOffset={8} className="w-[min(36rem,calc(100vw-2rem))] max-w-[calc(100vw-2rem)] p-4">
      <PopoverTitle>Time range</PopoverTitle>
      <div className="mt-3 grid gap-4 sm:grid-cols-[9rem_1fr]">
        <div className="flex flex-col gap-1">{presets.map(([name, value]) => <Button key={value} variant="ghost" className="justify-start" onClick={() => apply(value, "now")}>{name}</Button>)}</div>
        <form className="grid gap-2" onSubmit={(event) => { event.preventDefault(); apply(); }}>
          <Label htmlFor={`${id}-from`}>From</Label><Input id={`${id}-from`} value={from} onValueChange={setFrom} aria-describedby={`${id}-help`} />
          <Label htmlFor={`${id}-to`}>To</Label><Input id={`${id}-to`} value={to} onValueChange={setTo} aria-describedby={`${id}-help`} />
          <p id={`${id}-help`} className="text-xs text-muted-foreground">Relative: now-15m, now-1h, now-7d. Absolute: ISO timestamp with timezone, or epoch milliseconds.</p>
          <details className="text-sm">
            <summary className="cursor-pointer py-2">Choose absolute dates</summary>
            <p className="mb-2 text-xs text-muted-foreground">Browser timezone: {Intl.DateTimeFormat().resolvedOptions().timeZone}</p>
            <Label htmlFor={`${id}-calendar-from`}>Start date and time</Label><Input id={`${id}-calendar-from`} type="datetime-local" step="1" value={localDate(from)} onValueChange={(value) => { const date = new Date(value); if (Number.isFinite(date.getTime())) setFrom(date.toISOString()); }} />
            <Label htmlFor={`${id}-calendar-to`}>End date and time</Label><Input id={`${id}-calendar-to`} type="datetime-local" step="1" value={localDate(to)} onValueChange={(value) => { const date = new Date(value); if (Number.isFinite(date.getTime())) setTo(date.toISOString()); }} />
          </details>
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
          <div className="flex justify-end gap-2"><PopoverClose>Cancel</PopoverClose><Button type="submit">Apply range</Button></div>
        </form>
      </div>
    </PopoverContent>
  </Popover>;
}
