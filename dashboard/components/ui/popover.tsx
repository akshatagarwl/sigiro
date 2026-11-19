"use client";
import * as Sdk from "@ui-sdk/react/popover";
import * as React from "react";
export const Popover = Sdk.Popover;
export function PopoverTrigger({ asChild, children, ...props }: React.ComponentProps<typeof Sdk.PopoverTrigger> & { asChild?: boolean }) {
  const render = asChild && React.isValidElement<Record<string, unknown>>(children) ? children : undefined;
  return <Sdk.PopoverTrigger {...props} render={render}>{render ? undefined : children}</Sdk.PopoverTrigger>;
}
export const PopoverContent = Sdk.PopoverContent;
export const PopoverClose = Sdk.PopoverClose;
export const PopoverTitle = Sdk.PopoverTitle;
export const PopoverDescription = Sdk.PopoverDescription;
export function PopoverAnchor({ children }: { children?: React.ReactNode; asChild?: boolean }) { return children; }
