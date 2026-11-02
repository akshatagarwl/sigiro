"use client";

import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { DataTableFilterCommand } from "@/components/data-table/data-table-filter-command";
import { DataTableInfinite } from "@/components/data-table/data-table-infinite";
import { useDataTable } from "@/components/data-table/data-table-provider";
import { DataTableRefreshButton } from "@/components/data-table/data-table-refresh-button";
import { MemoizedDataTableSheetContent } from "@/components/data-table/data-table-sheet/data-table-sheet-content";
import { DataTableSheetDetails } from "@/components/data-table/data-table-sheet/data-table-sheet-details";
import { EvilBarChart } from "@/components/evilcharts/charts/recharts-bar-chart";
import type { ChartConfig } from "@/components/evilcharts/ui/recharts-chart";
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
import { type SqlSource, sqlTransport } from "@/lib/sigiro/table-source";
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

const STATE_KEYS = new Set(["sort", "uuid", "size", "direction", "cursor"]);
const DIRECTIONS = ["prev", "next"] as const;

type Explorer<TRow> = {
  id: string;
  // The table-schema definition; typed loosely because each table infers its own.
  definition: Parameters<typeof generateColumns>[0];
  source: Omit<SqlSource<TRow>, "window">;
  chart: ChartConfig;
  rowId: (row: TRow) => string;
  title: (row: TRow) => string;
  rowClassName?: (row: TRow) => string;
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
    direction: field.stringLiteral(DIRECTIONS).default("next"),
    cursor: field.timestamp(),
  });
  const { searchParamsParser, searchParamsSerializer } = createNuqsSearchParams(
    filterSchema.definition as SchemaDefinition
  );
  const getRowClassName = explorer.rowClassName
    ? (row: { original: TRow }) => explorer.rowClassName!(row.original)
    : undefined;
  const seriesKeys = Object.keys(explorer.chart);

  function useOptions() {
    const { from, to, range } = useControls();
    return React.useMemo(
      () =>
        createDataTableQueryOptions<TRow[], unknown>({
          queryKeyPrefix: `${explorer.id}-${range}`,
          apiEndpoint: `/${explorer.id}`,
          searchParamsSerializer: searchParamsSerializer as (
            search: Record<string, unknown>
          ) => string,
          skipMetaOnPagination: true,
          transport: sqlTransport<TRow>(
            { ...explorer.source, window: () => ({ from, to }) },
            searchParamsParser as never
          ),
          queryOptions: { retry: false },
        }),
      // `from`/`to` move on every tick; the key carries `range` and the
      // refresh below re-reads, so only the range rebuilds the options.
      // eslint-disable-next-line react-hooks/exhaustive-deps
      [range]
    );
  }

  function Table() {
    const search = useFilterState<Record<string, unknown>>();
    const options = useOptions();
    const queryClient = useQueryClient();
    const { tick } = useControls();
    const { data, isFetching, isLoading, fetchNextPage, hasNextPage } =
      useInfiniteQuery(options(search));

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
      <DataTableInfinite
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
        isFetching={isFetching}
        isLoading={isLoading}
        fetchNextPage={fetchNextPage}
        hasNextPage={hasNextPage}
        getRowId={explorer.rowId}
        getRowClassName={getRowClassName}
        getFacetedUniqueValues={getFacetedUniqueValues(facets)}
        getFacetedMinMaxValues={getFacetedMinMaxValues(facets)}
        chartSlot={
          <div className="h-36 w-full" data-chart-name="timeline">
            <EvilBarChart
              data={chartData}
              config={explorer.chart as never}
              stackType="stacked"
              className="h-full w-full"
              isLoading={isLoading}
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
          />
        }
        sheetSlot={
          <SheetSlot
            totalRows={meta?.totalRowCount ?? 0}
            filterRows={meta?.filterRowCount ?? 0}
            totalRowsFetched={flatData.length}
          />
        }
        tableId={explorer.id}
      />
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
