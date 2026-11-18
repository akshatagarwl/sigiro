"use client";
import { Calendar as SdkCalendar } from "@ui-sdk/react/calendar";
import { format } from "date-fns";
export type DateRange = { from: Date | undefined; to?: Date };
export function Calendar({ selected, onSelect, defaultMonth, autoFocus: _autoFocus, numberOfMonths = 1, mode: _mode, className }: { selected?: DateRange; onSelect?: (value: DateRange | undefined) => void; defaultMonth?: Date; autoFocus?: boolean; numberOfMonths?: number; mode?: "range"; className?: string }) {
  return <SdkCalendar mode="range" aria-label="Choose date range" className={className} months={numberOfMonths} defaultMonth={defaultMonth ? format(defaultMonth, "yyyy-MM") : undefined} value={selected?.from && selected.to ? [format(selected.from, "yyyy-MM-dd"), format(selected.to, "yyyy-MM-dd")] : null} onValueChange={(value) => { if (Array.isArray(value)) onSelect?.({ from: new Date(`${value[0]}T00:00:00`), to: new Date(`${value[1]}T00:00:00`) }); else if (value === null) onSelect?.(undefined); }} />;
}
