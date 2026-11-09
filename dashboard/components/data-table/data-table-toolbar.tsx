"use client";

import { useDataTable } from "@/components/data-table/data-table-provider";
import { Button } from "@/components/ui/button";
import { Button as ExplorerButton } from "@/components/observability/primitives";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { useHotKey } from "@/hooks/use-hot-key";
import { formatCompactNumber } from "@/lib/format";
import { useControls } from "@/components/controls";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { DataTableFilterControlsDrawer } from "./data-table-filter-controls-drawer";
import { DataTableResetButton } from "./data-table-reset-button";
import { DataTableViewOptions } from "./data-table-view-options";
import { TOOLTIP_DELAY } from "./ui-compat";

interface DataTableToolbarProps {
  renderActions?: () => React.ReactNode;
  showViewOptions?: boolean;
  showResetButton?: boolean;
  compact?: boolean;
  countLabel?: "in window" | "loaded";
}

export function DataTableToolbar({
  renderActions,
  showViewOptions = true,
  showResetButton = true,
  compact = false,
  countLabel = "in window",
}: DataTableToolbarProps) {
  const { table, filterFields, isLoading, columnFilters, totalRows, filterRows } = useDataTable();
  const { open, setOpen } = useControls();
  // Compact dashboard views reserve Mod+B for navigation, not both panels.
  useHotKey(() => {
    if (!compact) setOpen((prev) => !prev);
  }, "b");
  const rows = {
    total: totalRows ?? table.getCoreRowModel().rows.length,
    filtered: filterRows ?? table.getFilteredRowModel().rows.length,
  };

  if (compact)
    return (
      <div className="flex flex-wrap items-center gap-2" data-log-query-context>
        <div className="hidden sm:block">
          <ExplorerButton aria-expanded={open} onClick={() => setOpen(!open)}>
            Filters
          </ExplorerButton>
        </div>
        <div className="sm:hidden">
          <DataTableFilterControlsDrawer showLabel shortcut={false} />
        </div>
        <p className="text-muted-foreground text-xs" aria-live="polite">
          {isLoading
            ? "Updating…"
            : `${formatCompactNumber(rows.filtered)} matching · ${formatCompactNumber(rows.total)} ${countLabel}`}
        </p>
        {columnFilters.length > 0 && (
          <div className="flex flex-wrap gap-1.5" aria-label="Active filters">
            {columnFilters.map((filter) => {
              const label =
                filterFields.find((field) => String(field.value) === filter.id)?.label ?? filter.id;
              const values = Array.isArray(filter.value) ? filter.value : [filter.value];
              const delimiter = values.every((value) => value instanceof Date) ? " → " : ", ";
              const display = values
                .map((value) =>
                  value instanceof Date
                    ? value.toLocaleString(undefined, {
                        month: "short",
                        day: "numeric",
                        hour: "2-digit",
                        minute: "2-digit",
                      })
                    : String(value),
                )
                .join(delimiter);
              const fullValue = values
                .map((value) => (value instanceof Date ? value.toISOString() : String(value)))
                .join(delimiter);
              return (
                <ExplorerButton
                  key={filter.id}
                  aria-label={`Remove ${label}: ${display}`}
                  title={fullValue}
                  onClick={() => table.getColumn(filter.id)?.setFilterValue(undefined)}
                >
                  <span className="max-w-64 truncate">
                    {label}: {display}
                  </span>
                  <span aria-hidden="true">×</span>
                </ExplorerButton>
              );
            })}
          </div>
        )}
        <div className="ml-auto flex items-center gap-2">
          {showResetButton && columnFilters.length > 0 && <DataTableResetButton />}
          {renderActions?.()}
          {showViewOptions && <DataTableViewOptions />}
        </div>
      </div>
    );

  return (
    <div className="flex flex-wrap items-center justify-between gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <TooltipProvider {...TOOLTIP_DELAY}>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                onClick={() => setOpen((prev) => !prev)}
                className="hidden gap-2 sm:flex"
              >
                {open ? (
                  <>
                    <PanelLeftClose className="h-4 w-4" />
                    <span className="hidden md:block">Hide Controls</span>
                  </>
                ) : (
                  <>
                    <PanelLeftOpen className="h-4 w-4" />
                    <span className="hidden md:block">Show Controls</span>
                  </>
                )}
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <p className="text-nowrap">
                Toggle controls with{" "}
                <Kbd className="text-muted-foreground group-hover:text-accent-foreground ml-1">
                  <span className="mr-1">⌘</span>
                  <span>B</span>
                </Kbd>
              </p>
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
        <div className="block sm:hidden">
          <DataTableFilterControlsDrawer />
        </div>
        <div>
          <p className="text-muted-foreground hidden text-sm sm:block">
            <span className="font-mono font-medium">{formatCompactNumber(rows.filtered)}</span> of{" "}
            <span className="font-mono font-medium">{formatCompactNumber(rows.total)}</span> row(s){" "}
            <span className="sr-only sm:not-sr-only">filtered</span>
          </p>
          <p className="text-muted-foreground block text-sm sm:hidden">
            <span className="font-mono font-medium">{formatCompactNumber(rows.filtered)}</span>{" "}
            row(s)
          </p>
        </div>
      </div>
      <div className="ml-auto flex items-center gap-2">
        {columnFilters.length ? <DataTableResetButton /> : null}
        {renderActions?.()}
        {showViewOptions && <DataTableViewOptions />}
      </div>
    </div>
  );
}
