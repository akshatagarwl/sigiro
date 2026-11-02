"use client";

import { LogsTable } from "@/components/sigiro/logs-table";

export default function LogsPage() {
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-table="logs">
      <LogsTable />
    </div>
  );
}
