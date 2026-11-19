"use client";
import * as Sdk from "@ui-sdk/react/command";
import type * as React from "react";
import { Dialog, DialogContent, DialogTitle } from "./dialog";
export function Command({ filter, shouldFilter, ...props }: Omit<React.ComponentProps<typeof Sdk.Command>, "filter"> & { shouldFilter?: boolean; filter?: (value: string, query: string, keywords?: string[]) => number }) {
  return <Sdk.Command {...props} filter={shouldFilter === false ? false : filter ? (item, query) => filter(item.value, query, item.keywords) : undefined} />;
}
export function CommandInput(props: React.ComponentProps<typeof Sdk.CommandInput>) { return <Sdk.CommandInput aria-label="Search commands" {...props} />; }
export const CommandList = Sdk.CommandList;
export const CommandEmpty = Sdk.CommandEmpty;
export const CommandSeparator = Sdk.CommandSeparator;
export function CommandItem({ value, onSelect, ...props }: Omit<React.ComponentProps<typeof Sdk.CommandItem>, "onSelect"> & { onSelect?: (value: string) => unknown }) { return <Sdk.CommandItem {...props} value={value} onSelect={() => onSelect?.(value ?? "")} />; }
export function CommandGroup({ heading, children, ...props }: Omit<React.ComponentProps<typeof Sdk.CommandGroup>, "children"> & { children?: React.ReactNode; heading?: React.ReactNode }) { return <Sdk.CommandGroup {...props}>{heading && <Sdk.CommandGroupLabel>{heading}</Sdk.CommandGroupLabel>}{children}</Sdk.CommandGroup>; }
export function CommandShortcut(props: React.ComponentProps<"span">) { return <span {...props} className="ml-auto text-xs text-muted-foreground" />; }
export function CommandDialog({ children, title = "Commands", ...props }: React.ComponentProps<typeof Dialog> & { title?: string }) { return <Dialog {...props}><DialogContent><DialogTitle>{title}</DialogTitle><Command>{children}</Command></DialogContent></Dialog>; }
