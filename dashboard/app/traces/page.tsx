"use client";

import { useSearchParams, useRouter } from "next/navigation";
import * as React from "react";

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
  const trace = params.get("trace") ?? params.get("uuid");
  const heading = React.useRef<HTMLHeadingElement>(null);
  const returnId = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (trace) returnId.current = trace;
  }, [trace]);
  const close = () => {
    const next = new URLSearchParams(params);
    next.delete("trace");
    next.delete("uuid");
    next.delete("span");
    router.replace(`/traces/${next.size > 0 ? `?${next}` : ""}`);
  };
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-page="traces" data-table="traces">
      <div className="shrink-0 px-4 pt-4 lg:px-6">
        <h2 ref={heading} tabIndex={-1} className="text-lg font-semibold">
          Trace explorer
        </h2>
        <p className="text-muted-foreground mt-1 text-sm">
          Find a run, follow its spans, and inspect the signals behind it.
        </p>
      </div>
      <TracesTable />
      <Sheet open={Boolean(trace)} onOpenChange={(open) => !open && close()}>
        <SheetContent
          className="w-full overflow-y-auto sm:max-w-[96vw]"
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            const row = returnId.current ? document.getElementById(returnId.current) : null;
            (row ?? heading.current)?.focus({ preventScroll: true });
          }}
        >
          <SheetHeader>
            <SheetTitle>Trace detail</SheetTitle>
            <SheetDescription className="font-mono">{trace}</SheetDescription>
          </SheetHeader>
          <div className="min-h-0 px-4 pb-4">{trace && <TraceView traceId={trace} />}</div>
        </SheetContent>
      </Sheet>
    </div>
  );
}
