"use client";
import * as Sdk from "@ui-sdk/react/drawer";
import type * as React from "react";
import { childElement } from "./composition";
export const Drawer = Sdk.Drawer;
export const DrawerTitle = Sdk.DrawerTitle;
export const DrawerDescription = Sdk.DrawerDescription;
export function DrawerTrigger({ asChild, children, render, ...props }: React.ComponentProps<typeof Sdk.DrawerTrigger> & { asChild?: boolean }) { return <Sdk.DrawerTrigger {...props} render={childElement(children, asChild) ?? render}>{asChild ? undefined : children}</Sdk.DrawerTrigger>; }
export function DrawerClose({ asChild, children, render, ...props }: React.ComponentProps<typeof Sdk.DrawerClose> & { asChild?: boolean }) { return <Sdk.DrawerClose {...props} render={childElement(children, asChild) ?? render}>{asChild ? undefined : children}</Sdk.DrawerClose>; }
export function DrawerContent(props: React.ComponentProps<typeof Sdk.DrawerContent>) { return <><Sdk.DrawerBackdrop /><Sdk.DrawerContent {...props} /></>; }
export { DialogHeader as DrawerHeader, DialogFooter as DrawerFooter } from "./dialog";
