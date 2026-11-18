"use client";
import * as Sdk from "@ui-sdk/react/menu";
import * as React from "react";
import { childElement } from "./composition";
export const DropdownMenu = Sdk.Menu;
export const DropdownMenuContent = Sdk.MenuContent;
export const DropdownMenuGroup = Sdk.MenuGroup;
export const DropdownMenuSeparator = Sdk.MenuSeparator;
export const DropdownMenuRadioGroup = Sdk.MenuRadioGroup;
export const DropdownMenuRadioItem = Sdk.MenuRadioItem;
export const DropdownMenuSub = Sdk.MenuSub;
export const DropdownMenuSubTrigger = Sdk.MenuSubTrigger;
export const DropdownMenuSubContent = Sdk.MenuSubContent;
export function DropdownMenuTrigger({ asChild, children, render, ...props }: Omit<React.ComponentProps<typeof Sdk.MenuTrigger>, "children"> & { children?: React.ReactNode; asChild?: boolean }) { return <Sdk.MenuTrigger {...props} render={childElement(children, asChild) ?? render}>{asChild ? undefined : children}</Sdk.MenuTrigger>; }
export function DropdownMenuItem({ value, inset: _inset, variant: _variant, ...props }: Omit<React.ComponentProps<typeof Sdk.MenuItem>, "value"> & { value?: string; inset?: boolean; variant?: "default" | "destructive" }) { const id = React.useId(); return <Sdk.MenuItem {...props} value={value ?? id} />; }
export function DropdownMenuCheckboxItem({ children, ...props }: Omit<React.ComponentProps<typeof Sdk.MenuCheckboxItem>, "children"> & { children?: React.ReactNode }) { return <Sdk.MenuCheckboxItem {...props}><Sdk.MenuItemIndicator />{children}</Sdk.MenuCheckboxItem>; }
export function DropdownMenuLabel({ inset: _inset, ...props }: React.ComponentProps<typeof Sdk.MenuGroupLabel> & { inset?: boolean }) { return <Sdk.MenuGroupLabel {...props} />; }
export function DropdownMenuShortcut(props: React.ComponentProps<"span">) { return <span {...props} className="ml-auto font-mono text-xs text-muted-foreground" />; }
