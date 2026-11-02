"use client";

import { useSearchParams, useRouter } from "next/navigation";

import { TraceView } from "@/components/sigiro/trace-view";
import { TracesTable } from "@/components/sigiro/traces-table";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";

/**
 * The trace list, plus `?trace=<id>` for a link to one trace (from a log line
 * or elsewhere) that may be outside the list's window.
 */
export default function TracesPage() {
  const params = useSearchParams();
  const router = useRouter();
  const trace = params.get("trace");
  const close = () => {
    const next = new URLSearchParams(params);
    next.delete("trace");
    router.replace(`/traces/${next.size > 0 ? `?${next}` : ""}`);
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-table="traces">
      <TracesTable />
      <Sheet open={Boolean(trace)} onOpenChange={(open) => !open && close()}>
        <SheetContent className="w-full sm:max-w-2xl">
          <SheetHeader>
            <SheetTitle>Trace</SheetTitle>
            <SheetDescription className="font-mono">{trace}</SheetDescription>
          </SheetHeader>
          <div className="px-4">{trace && <TraceView traceId={trace} />}</div>
        </SheetContent>
      </Sheet>
    </div>
  );
}
