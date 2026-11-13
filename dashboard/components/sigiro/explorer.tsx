"use client";

import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { DataTableFilterCommand } from "@/components/data-table/data-table-filter-command";
import { DataTableInfinite } from "@/components/data-table/data-table-infinite";
import { useDataTable } from "@/components/data-table/data-table-provider";
import { DataTableRefreshButton } from "@/components/data-table/data-table-refresh-button";
import { MemoizedDataTableSheetContent } from "@/components/data-table/data-table-sheet/data-table-sheet-content";
import { DataTableSheetDetails } from "@/components/data-table/data-table-sheet/data-table-sheet-details";
import { canLoadMore } from "@/components/data-table/utils";
import { EvilBarChart } from "@/components/evilcharts/charts/recharts-bar-chart";
import type { ChartConfig } from "@/components/evilcharts/ui/recharts-chart";
import { RequestState } from "@/components/observability/request-state";
import { Button } from "@/components/ui/button";
import {
  applyFacets,
  createDataTableQueryOptions,
  getFacetedMinMaxValues,
  getFacetedUniqueValues,
  getMetaPage,
  refreshDataTableQuery,
} from "@/lib/data-table";
import { isActive } from "@/lib/filters";
import { useControls } from "@/lib/sigiro/controls";
import {
  type ExplorerView,
  explorerView,
  type MoreState,
} from "@/lib/sigiro/explorer-state";
import {
  keysetPagination,
  type SqlSource,
  sqlTransport,
} from "@/lib/sigiro/table-source";
import { useNuqsAdapter } from "@/lib/store/adapters/nuqs";
import { createNuqsSearchParams } from "@/lib/store/adapters/nuqs/server";
import { useFilterState } from "@/lib/store/hooks/useFilterState";
import { DataTableStoreProvider } from "@/lib/store/provider/DataTableStoreProvider";
import { field } from "@/lib/store/schema";
import type { SchemaDefinition } from "@/lib/store/schema/types";
import {
  generateColumns,
  generateFilterFields,
  generateFilterSchema,
  generateSheetFields,
  getDefaultColumnVisibility,
} from "@/lib/table-schema";

/**
 * The openstatus data-table-filters infinite table (its
 * `data-table-example-infinite` client, generalised to a schema), reading
 * from `/v1/query` through `sqlTransport`. The timeline above it is an
 * EvilCharts stacked bar chart instead of the block's own recharts chart.
 */

const STATE_KEYS = new Set([
  "sort",
  "uuid",
  "size",
  "direction",
  "cursor",
  "after",
]);
const DIRECTIONS = ["prev", "next"] as const;

/** The load-more control's slice of {@link ExplorerView}. */
export type ExplorerPagination = {
  /** The whole results view, for empty/failed slots. */
  view: ExplorerView;
  more: MoreState | null;
  onLoadMore: () => void;
};

type Explorer<TRow> = {
  id: string;
  // The table-schema definition; typed loosely because each table infers its own.
  definition: Parameters<typeof generateColumns>[0];
  source: Omit<SqlSource<TRow>, "window">;
  chart: ChartConfig;
  rowId: (row: TRow) => string;
  title: (row: TRow) => string;
  rowClassName?: (row: TRow) => string;
  detail?: boolean;
  renderResults?: (
    rows: TRow[],
    pagination: ExplorerPagination
  ) => React.ReactNode;
  filterOverrides?: SchemaDefinition;
};

export function createExplorer<TRow extends { timestamp: Date }>(
  explorer: Explorer<TRow>
) {
  const columns = generateColumns<TRow>(explorer.definition);
  const filterFields = generateFilterFields<TRow>(explorer.definition);
  const sheetFields = generateSheetFields<TRow>(explorer.definition);
  const defaultColumnVisibility = getDefaultColumnVisibility(
    explorer.definition
  );
  const filterSchema = generateFilterSchema(explorer.definition, {
    sort: field.sort(),
    uuid: field.string(),
    size: field.number().default(50),
    ...explorer.filterOverrides,
    direction: field.stringLiteral(DIRECTIONS).default("next"),
    cursor: field.timestamp(),
    after: field.string(),
  });
  const { searchParamsParser, searchParamsSerializer } = createNuqsSearchParams(
    filterSchema.definition as SchemaDefinition
  );
  const getRowClassName = explorer.rowClassName
    ? (row: { original: TRow }) => explorer.rowClassName!(row.original)
    : undefined;
  const seriesKeys = Object.keys(explorer.chart);

  function useOptions() {
    const { from, to, windowKey } = useControls();
    return React.useMemo(
      () =>
        createDataTableQueryOptions<TRow[], unknown>({
          queryKeyPrefix: `${explorer.id}-${windowKey}`,
          apiEndpoint: `/${explorer.id}`,
          searchParamsSerializer: searchParamsSerializer as (
            search: Record<string, unknown>
          ) => string,
          skipMetaOnPagination: true,
          pagination: keysetPagination(),
          transport: sqlTransport<TRow>(
            { ...explorer.source, window: () => ({ from, to }) },
            searchParamsParser as never
          ),
          queryOptions: { retry: false },
        }),
      // Keep the query identity stable within a range, but rebuild its fetch
      // closure when Refresh/live advances the fallback window.
      [windowKey, from, to]
    );
  }

  function Table() {
    const search = useFilterState<Record<string, unknown>>();
    const options = useOptions();
    const queryClient = useQueryClient();
    const { tick } = useControls();
    const {
      data,
      error,
      status,
      isFetching,
      isFetchingNextPage,
      isFetchNextPageError,
      fetchNextPage,
      hasNextPage,
    } = useInfiniteQuery({
      ...options(search),
      placeholderData: undefined,
    });

    const refresh = React.useCallback(
      () => refreshDataTableQuery(queryClient, options(search)),
      [queryClient, options, search]
    );
    const first = React.useRef(true);
    React.useEffect(() => {
      if (first.current) {
        first.current = false;
        return;
      }
      void refresh();
      // Only the header's refresh/live tick drives this.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [tick]);

    const flatData = React.useMemo(
      () => data?.pages?.flatMap((page) => page.data ?? []) ?? [],
      [data?.pages]
    );
    const meta = getMetaPage(data)?.meta;
    const facets = meta?.facets;
    const defaultColumnFilters = Object.entries(search)
      .filter(([key, value]) => !STATE_KEYS.has(key) && isActive(value))
      .map(([id, value]) => ({ id, value }));
    const dynamicFilterFields = React.useMemo(
      () => applyFacets(filterFields, facets),
      [facets]
    );
    const view = explorerView({
      rows: flatData.length,
      status,
      error,
      isFetching,
      isFetchingNextPage,
      isFetchNextPageError,
      hasMore: canLoadMore({
        hasNextPage,
        filterRows: meta?.filterRowCount,
        totalRowsFetched: flatData.length,
      }),
    });
    const loadMore = React.useCallback(() => {
      void fetchNextPage();
    }, [fetchNextPage]);
    const chartData = React.useMemo(
      () =>
        (meta?.chartData ?? []).map((point) => ({
          ...point,
          time: new Date(point.timestamp).toLocaleString(undefined, {
            month: "short",
            day: "numeric",
            hour: "2-digit",
            minute: "2-digit",
          }),
        })),
      [meta?.chartData]
    );

    return (
      <>
        {view.kind === "failed" && (
          <RequestState
            kind="error"
            title={`Unable to load ${explorer.id}`}
            onRetry={refresh}
            retrying={view.retrying}
          >
            {view.error.message}
          </RequestState>
        )}
        {view.kind === "rows" && view.refreshError && (
          <RequestState
            kind="error"
            title={`Unable to refresh ${explorer.id}`}
            onRetry={refresh}
            retrying={view.refreshing}
          >
            {view.refreshError.message} Showing previously loaded results until
            retry succeeds.
          </RequestState>
        )}
        <DataTableInfinite
          compact
          emptySlot={
            view.kind === "loading" || (view.kind === "failed" && view.retrying)
              ? "Loading results…"
              : view.kind === "failed"
                ? "Results are unavailable until the request succeeds."
                : `No matching ${explorer.id}. Adjust the time range or filters.`
          }
          resultsSlot={explorer.renderResults?.(flatData, {
            view,
            more: view.kind === "rows" ? view.more : null,
            onLoadMore: loadMore,
          })}
          loadMoreSlot={
            view.kind === "rows" && view.more === "failed" ? (
              <LoadMoreFailed
                error={view.moreError}
                onRetry={loadMore}
                label={explorer.id}
              />
            ) : undefined
          }
          columns={columns}
          data={flatData}
          totalRows={meta?.totalRowCount}
          filterRows={meta?.filterRowCount}
          totalRowsFetched={flatData.length}
          defaultColumnFilters={defaultColumnFilters}
          defaultColumnSorting={
            search.sort
              ? [search.sort as { id: string; desc: boolean }]
              : undefined
          }
          defaultRowSelection={
            search.uuid ? { [String(search.uuid)]: true } : undefined
          }
          defaultColumnVisibility={defaultColumnVisibility}
          filterFields={dynamicFilterFields}
          isFetching={view.kind === "rows" ? view.more === "loading" : false}
          isLoading={view.kind === "loading"}
          fetchNextPage={fetchNextPage}
          hasNextPage={hasNextPage}
          getRowId={explorer.rowId}
          getRowClassName={getRowClassName}
          getFacetedUniqueValues={getFacetedUniqueValues(facets)}
          getFacetedMinMaxValues={getFacetedMinMaxValues(facets)}
          chartSlot={
            <div
              className={
                explorer.renderResults ? "h-16 w-full sm:h-20" : "h-36 w-full"
              }
              data-chart-name="timeline"
            >
              <EvilBarChart
                data={chartData}
                config={explorer.chart as never}
                stackType="stacked"
                className="h-full w-full"
                isLoading={view.kind === "loading"}
              >
                <EvilBarChart.Grid />
                <EvilBarChart.XAxis dataKey="time" />
                <EvilBarChart.Tooltip />
                {seriesKeys.map((key) => (
                  <EvilBarChart.Bar key={key} dataKey={key} />
                ))}
              </EvilBarChart>
            </div>
          }
          toolbarActions={
            <DataTableRefreshButton onClick={() => void refresh()} />
          }
          commandSlot={
            <DataTableFilterCommand
              schema={filterSchema.definition}
              tableId={explorer.id}
              placeholder={
                explorer.id === "logs"
                  ? "Filter logs… e.g. service:api"
                  : undefined
              }
            />
          }
          sheetSlot={
            explorer.detail === false ? null : (
              <SheetSlot
                totalRows={meta?.totalRowCount ?? 0}
                filterRows={meta?.filterRowCount ?? 0}
                totalRowsFetched={flatData.length}
              />
            )
          }
          tableId={explorer.id}
        />
      </>
    );
  }

  function LoadMoreFailed({
    error,
    onRetry,
    label,
  }: {
    error: Error | null;
    onRetry: () => void;
    label: string;
  }) {
    return (
      <div
        role="alert"
        className="flex flex-wrap items-center justify-center gap-2 text-sm"
      >
        <span className="text-muted-foreground">
          Could not load more {label}
          {error ? `: ${error.message}` : "."}
        </span>
        <Button variant="outline" size="sm" onClick={onRetry}>
          Retry
        </Button>
      </div>
    );
  }

  function SheetSlot(metadata: {
    totalRows: number;
    filterRows: number;
    totalRowsFetched: number;
  }) {
    const { table, rowSelection, isLoading, filterFields } = useDataTable<
      TRow,
      unknown
    >();
    const selectedRowKey = Object.keys(rowSelection)?.[0];
    const selectedRow = React.useMemo(() => {
      if (isLoading && !selectedRowKey) return undefined;
      return table
        .getCoreRowModel()
        .flatRows.find((row) => row.id === selectedRowKey);
    }, [selectedRowKey, isLoading, table]);

    return (
      <DataTableSheetDetails
        title={selectedRow ? explorer.title(selectedRow.original) : undefined}
        titleClassName="font-mono"
      >
        <MemoizedDataTableSheetContent
          table={table}
          data={selectedRow?.original}
          filterFields={filterFields}
          fields={sheetFields}
          metadata={metadata}
        />
      </DataTableSheetDetails>
    );
  }

  return function ExplorerTable() {
    const adapter = useNuqsAdapter(filterSchema.definition, {
      id: explorer.id,
    });
    return (
      <DataTableStoreProvider adapter={adapter}>
        <Table />
      </DataTableStoreProvider>
    );
  };
}
