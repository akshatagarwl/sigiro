"use client";

import { DataTableFilterCommand } from "@/components/data-table/data-table-filter-command";
import { DataTableInfinite } from "@/components/data-table/data-table-infinite";
import { useDataTable } from "@/components/data-table/data-table-provider";
import { MemoizedDataTableSheetContent } from "@/components/data-table/data-table-sheet/data-table-sheet-content";
import { DataTableSheetDetails } from "@/components/data-table/data-table-sheet/data-table-sheet-details";
import type { SheetField } from "@/components/data-table/types";
import { useMemoryAdapter } from "@/lib/store/adapters/memory";
import { DataTableStoreProvider } from "@/lib/store/provider/DataTableStoreProvider";
import { field } from "@/lib/store/schema";
import type { SchemaDefinition } from "@/lib/store/schema/types";
import {
  createTableSchema,
  generateColumns,
  generateFilterFields,
  generateFilterSchema,
  generateSheetFields,
  getDefaultColumnVisibility,
} from "@/lib/table-schema";
import { inferSchemaFromJSON } from "@/lib/table-schema/infer";
import * as React from "react";

type AutoRow = Record<string, unknown>;

export interface DataTableAutoProps {
  data: AutoRow[];
  tableId?: string;
  compact?: boolean;
  isUpdating?: boolean;
  getRowId?: (row: AutoRow, index: number) => string;
}

export function DataTableAuto({
  data,
  tableId = "auto",
  compact = false,
  isUpdating = false,
  getRowId,
}: DataTableAutoProps) {
  const schemaJson = React.useMemo(() => inferSchemaFromJSON(data), [data]);

  const { definition } = React.useMemo(() => createTableSchema.fromJSON(schemaJson), [schemaJson]);

  const columns = React.useMemo(() => generateColumns<AutoRow>(definition), [definition]);

  const filterFields = React.useMemo(() => generateFilterFields<AutoRow>(definition), [definition]);

  const sheetFields = React.useMemo(() => generateSheetFields<AutoRow>(definition), [definition]);

  const defaultColumnVisibility = React.useMemo(
    () => getDefaultColumnVisibility(definition),
    [definition],
  );

  // Merge `sort` BEFORE createSchema so `defaults` cannot disagree with
  // `definition` — splicing it in afterwards left `definition.sort` present
  // while `defaults.sort` was missing.
  const filterSchema = React.useMemo(
    () => generateFilterSchema(definition, { sort: field.sort() }),
    [definition],
  );

  const adapter = useMemoryAdapter(filterSchema.definition, { id: tableId });

  return (
    <DataTableStoreProvider adapter={adapter}>
      <DataTableAutoInner
        tableId={tableId}
        compact={compact}
        isUpdating={isUpdating}
        getRowId={getRowId}
        data={data}
        columns={columns}
        filterFields={filterFields}
        sheetFields={sheetFields}
        defaultColumnVisibility={defaultColumnVisibility}
        schema={filterSchema.definition}
      />
    </DataTableStoreProvider>
  );
}

const noop = () => Promise.resolve();

function DataTableAutoInner({
  data,
  columns,
  filterFields,
  sheetFields,
  defaultColumnVisibility,
  schema,
  tableId,
  compact,
  isUpdating,
  getRowId,
}: DataTableAutoProps & {
  tableId: string;
  columns: React.ComponentProps<typeof DataTableInfinite<AutoRow>>["columns"];
  filterFields: React.ComponentProps<typeof DataTableInfinite<AutoRow>>["filterFields"];
  sheetFields: SheetField<AutoRow>[];
  defaultColumnVisibility: Record<string, boolean>;
  schema: SchemaDefinition;
}) {
  return (
    <DataTableInfinite
      compact={compact}
      countLabel="loaded"
      getRowId={getRowId}
      columns={columns}
      data={data}
      filterFields={filterFields}
      defaultColumnVisibility={defaultColumnVisibility}
      totalRows={data.length}
      totalRowsFetched={data.length}
      hasNextPage={false}
      fetchNextPage={noop}
      isFetching={isUpdating}
      isLoading={false}
      tableId={tableId}
      commandSlot={
        <DataTableFilterCommand
          schema={schema}
          tableId={tableId}
          placeholder="Filter loaded rows…"
        />
      }
      sheetSlot={<AutoSheetSlot sheetFields={sheetFields} data={data} getRowId={getRowId} />}
      footerSlot={
        <div className="text-muted-foreground text-sm">
          powered by{" "}
          <a
            href="https://openstatus.dev"
            target="_blank"
            rel="noreferrer"
            className="text-foreground hover:text-primary"
          >
            openstatus.dev
          </a>
        </div>
      }
    />
  );
}

function AutoSheetSlot({
  sheetFields: fields,
  data,
  getRowId,
}: Pick<DataTableAutoProps, "data" | "getRowId"> & {
  sheetFields: SheetField<AutoRow>[];
}) {
  const { table, rowSelection, filterFields } = useDataTable<AutoRow, unknown>();
  const selectedRowKey = Object.keys(rowSelection)?.[0];
  // The dataset, not the stable table instance, owns the selected payload.
  const selectedRow = React.useMemo(
    () => data.find((row, index) => (getRowId?.(row, index) ?? String(index)) === selectedRowKey),
    [data, getRowId, selectedRowKey],
  );

  return (
    <DataTableSheetDetails
      title={selectedRow ? String(Object.values(selectedRow)[0] ?? "") : "Selected row unavailable"}
      titleClassName="font-mono"
    >
      {!selectedRow && selectedRowKey ? (
        <p role="status" className="p-4 text-sm">
          This row is no longer in the current results. Close this panel to continue.
        </p>
      ) : (
        <MemoizedDataTableSheetContent
          table={table}
          data={selectedRow}
          filterFields={filterFields}
          fields={fields}
          metadata={{
            totalRows: data.length,
            filterRows: table.getFilteredRowModel().rows.length,
            totalRowsFetched: data.length,
          }}
        />
      )}
    </DataTableSheetDetails>
  );
}
