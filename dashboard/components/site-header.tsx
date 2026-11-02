"use client"

import { IconMoon, IconRefresh, IconSun } from "@tabler/icons-react"
import { useTheme } from "next-themes"
import { usePathname } from "next/navigation"

import { Button } from "@/components/ui/button"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Separator } from "@/components/ui/separator"
import { SidebarTrigger } from "@/components/ui/sidebar"
import { Toggle } from "@/components/ui/toggle"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { RANGES, type RangeKey, useControls, useSql } from "@/lib/sigiro/controls"
import { ts } from "@/lib/sigiro/query"

const ALL = "__all__"

/** Services seen in the window, for the header's service filter. */
function ServiceSelect() {
  const { service, setService } = useControls()
  const services = useSql(
    "service-list",
    (w) =>
      `SELECT DISTINCT service_name AS s FROM sigiro_spans WHERE timestamp >= ${ts(w.from)} AND timestamp < ${ts(w.to)} UNION SELECT DISTINCT service_name FROM sigiro_logs WHERE timestamp >= ${ts(w.from)} AND timestamp < ${ts(w.to)} ORDER BY 1`
  )
  const names = (services.data ?? [])
    .map((row) => String(row.s ?? ""))
    .filter(Boolean)
  if (service && !names.includes(service)) names.unshift(service)
  return (
    <Select
      value={service || ALL}
      onValueChange={(value) => setService(value === ALL ? "" : value)}
    >
      <SelectTrigger className="w-44" size="sm" aria-label="Service" data-action="service">
        <SelectValue />
      </SelectTrigger>
      <SelectContent className="rounded-xl">
        <SelectItem value={ALL} className="rounded-lg">
          All services
        </SelectItem>
        {names.map((name) => (
          <SelectItem key={name} value={name} className="rounded-lg">
            {name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

const TITLES: Record<string, string> = {
  "/": "Overview",
  "/services/": "Services",
  "/traces/": "Traces",
  "/logs/": "Logs",
  "/sql/": "SQL",
}

const KEYS = Object.keys(RANGES) as RangeKey[]

export function SiteHeader() {
  const pathname = usePathname()
  const { range, setRange, live, setLive, refresh } = useControls()
  const { resolvedTheme, setTheme } = useTheme()
  const pick = (value: string) => {
    const key = KEYS.find((known) => known === value)
    if (key) setRange(key)
  }

  return (
    <header className="flex h-(--header-height) shrink-0 items-center gap-2 border-b transition-[width,height] ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-(--header-height)">
      <div className="flex w-full items-center gap-1 px-4 lg:gap-2 lg:px-6">
        <SidebarTrigger className="-ml-1" />
        <Separator
          orientation="vertical"
          className="mx-2 data-[orientation=vertical]:h-4"
        />
        <h1 className="text-base font-medium">{TITLES[pathname] ?? "Sigiro"}</h1>
        <div className="ml-auto flex items-center gap-2">
          {(pathname === "/" || pathname === "/services/") && <ServiceSelect />}
          <ToggleGroup
            type="single"
            value={range}
            onValueChange={pick}
            variant="outline"
            aria-label="Time range"
            className="hidden *:data-[slot=toggle-group-item]:px-3! @container/main:flex lg:flex"
          >
            {KEYS.map((key) => (
              <ToggleGroupItem key={key} value={key} title={RANGES[key].label}>
                {key}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <Select value={range} onValueChange={pick}>
            <SelectTrigger
              className="flex w-36 lg:hidden"
              size="sm"
              aria-label="Time range"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent className="rounded-xl">
              {KEYS.map((key) => (
                <SelectItem key={key} value={key} className="rounded-lg">
                  {RANGES[key].label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Toggle
            variant="outline"
            size="sm"
            pressed={live}
            onPressedChange={setLive}
            aria-label="Live refresh every 10 seconds"
            data-action="live"
          >
            <span
              className={`size-2 rounded-full ${live ? "bg-success animate-pulse" : "bg-muted-foreground/40"}`}
            />
            Live
          </Toggle>
          <Button
            variant="outline"
            size="icon"
            className="size-8"
            onClick={refresh}
            aria-label="Refresh"
            data-action="refresh"
          >
            <IconRefresh />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="size-8"
            aria-label="Toggle theme"
            onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
          >
            {resolvedTheme === "dark" ? <IconSun /> : <IconMoon />}
          </Button>
        </div>
      </div>
    </header>
  )
}
