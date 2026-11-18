"use client";
import * as Sdk from "@ui-sdk/react/dialog";
import type * as React from "react";
import { cn } from "@/lib/utils";
import { childElement } from "./composition";
export const Dialog = Sdk.Dialog;
export const DialogTitle = Sdk.DialogTitle;
export const DialogDescription = Sdk.DialogDescription;
export const DialogOverlay = Sdk.DialogBackdrop;
export function DialogTrigger({ asChild, children, render, ...props }: React.ComponentProps<typeof Sdk.DialogTrigger> & { asChild?: boolean }) {
  const element = childElement(children, asChild) ?? render;
  return <Sdk.DialogTrigger {...props} render={element}>{asChild ? undefined : children}</Sdk.DialogTrigger>;
}
export function DialogClose({ asChild, children, render, ...props }: React.ComponentProps<typeof Sdk.DialogClose> & { asChild?: boolean }) {
  const element = childElement(children, asChild) ?? render;
  return <Sdk.DialogClose {...props} render={element}>{asChild ? undefined : children}</Sdk.DialogClose>;
}
export function DialogContent({ showCloseButton = true, children, ...props }: Omit<React.ComponentProps<typeof Sdk.DialogContent>, "children"> & { children?: React.ReactNode; showCloseButton?: boolean }) {
  return <><Sdk.DialogBackdrop /><Sdk.DialogContent {...props}>{children}{showCloseButton && <Sdk.DialogClose aria-label="Close dialog" className="absolute right-3 top-3">×</Sdk.DialogClose>}</Sdk.DialogContent></>;
}
export function DialogHeader({ className, ...props }: React.ComponentProps<"div">) { return <div {...props} className={cn("flex flex-col gap-2", className)} />; }
export function DialogFooter({ className, ...props }: React.ComponentProps<"div">) { return <div {...props} className={cn("flex justify-end gap-2", className)} />; }
