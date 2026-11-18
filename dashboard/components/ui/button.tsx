"use client";

import { Button as SdkButton } from "@ui-sdk/react/button";
import * as React from "react";
import { cn } from "@/lib/utils";

type Variant = "default" | "destructive" | "outline" | "secondary" | "ghost" | "link";
type Size = "default" | "xs" | "sm" | "lg" | "icon" | "icon-xs" | "icon-sm" | "icon-lg";
const sizes: Record<Size, string> = {
  default: "h-9 px-3", xs: "h-6 px-2 text-xs", sm: "h-8 px-2.5", lg: "h-10 px-4",
  icon: "size-9 p-0", "icon-xs": "size-6 p-0", "icon-sm": "size-8 p-0", "icon-lg": "size-10 p-0",
};
export function buttonVariants({ variant = "default", size = "default", className }: { variant?: Variant | null; size?: Size | null; className?: string } = {}) {
  return cn("inline-flex shrink-0 items-center justify-center gap-2 rounded-md border px-3 text-sm font-medium focus-visible:outline-2 focus-visible:outline-offset-2 disabled:opacity-50 [&_svg]:size-4", sizes[size ?? "default"], variant === "ghost" || variant === "link" ? "border-transparent bg-transparent hover:bg-muted" : variant === "secondary" ? "border-transparent bg-muted" : undefined, className);
}
export function Button({ variant, size, asChild, children, className, ...props }: Omit<React.ComponentProps<"button">, "onClick"> & Pick<React.ComponentProps<typeof SdkButton>, "onClick"> & { variant?: Variant; size?: Size; asChild?: boolean }) {
  const render = asChild && React.isValidElement<Record<string, unknown>>(children) ? children : undefined;
  const content = React.Children.map(children, (child) => typeof child === "string" || typeof child === "number" ? <span>{child}</span> : child);
  return <SdkButton {...props} data-variant={variant === undefined || variant === "default" ? "solid" : variant === "destructive" ? "destructive" : undefined} className={buttonVariants({ variant, size, className })} render={render}>{render ? undefined : content}</SdkButton>;
}
