"use client";
import * as Sdk from "@ui-sdk/react/dialog";
import type * as React from "react";
import { cn } from "@/lib/utils";
export { Dialog as Sheet, DialogTrigger as SheetTrigger, DialogClose as SheetClose, DialogTitle as SheetTitle, DialogDescription as SheetDescription, DialogHeader as SheetHeader, DialogFooter as SheetFooter } from "./dialog";
export function SheetContent({ side = "right", showCloseButton = true, hideClose = false, className, children, ...props }: Omit<React.ComponentProps<typeof Sdk.DialogContent>, "children"> & { children?: React.ReactNode; side?: "left" | "right" | "top" | "bottom"; showCloseButton?: boolean; hideClose?: boolean }) {
  const position = side === "left" ? "inset-y-0 left-0 right-auto" : side === "right" ? "inset-y-0 right-0 left-auto" : side === "top" ? "inset-x-0 top-0 bottom-auto" : "inset-x-0 bottom-0 top-auto";
  return <><Sdk.DialogBackdrop /><Sdk.DialogContent {...props} className={cn("m-0 flex translate-none flex-col rounded-none p-4", position, side === "left" || side === "right" ? "h-dvh max-h-dvh w-[min(48rem,95vw)] max-w-[95vw]" : "max-h-[90dvh] w-full max-w-full", className)}>{children}{showCloseButton && !hideClose && <Sdk.DialogClose aria-label="Close panel" className="absolute right-3 top-3">×</Sdk.DialogClose>}</Sdk.DialogContent></>;
}
