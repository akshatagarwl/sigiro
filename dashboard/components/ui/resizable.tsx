"use client";
import * as Sdk from "@ui-sdk/react/resizable";
import type * as React from "react";
export const ResizablePanelGroup = Sdk.Resizable;
type Size = number | `${number}%`;
const percent = (value: Size | undefined) => typeof value === "string" ? Number.parseFloat(value) : value;
export function ResizablePanel({ defaultSize, minSize, maxSize, ...props }: Omit<React.ComponentProps<typeof Sdk.ResizablePanel>, "defaultSize" | "minSize" | "maxSize"> & { defaultSize?: Size; minSize?: Size; maxSize?: Size }) {
  return <Sdk.ResizablePanel {...props} defaultSize={percent(defaultSize)} minSize={percent(minSize)} maxSize={percent(maxSize)} />;
}
export function ResizableHandle({ withHandle: _withHandle, ...props }: React.ComponentProps<typeof Sdk.ResizableHandle> & { withHandle?: boolean }) { return <Sdk.ResizableHandle aria-label="Resize panels" {...props} />; }
