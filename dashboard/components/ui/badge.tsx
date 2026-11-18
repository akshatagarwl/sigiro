"use client";
import { renderElement } from "@ui-sdk/react/primitive";
import type * as React from "react";
import { cn } from "@/lib/utils";
import { childElement } from "./composition";
export function badgeVariants({ variant = "default", className }: { variant?: "default" | "secondary" | "destructive" | "outline"; className?: string } = {}) { return cn("inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-xs font-medium", variant === "destructive" ? "text-destructive" : variant === "secondary" ? "bg-muted" : "bg-background", className); }
export function Badge({ asChild, variant, children, className, ...props }: React.ComponentProps<"span"> & { asChild?: boolean; variant?: "default" | "secondary" | "destructive" | "outline" }) { return renderElement("span", { ...props, className: badgeVariants({ variant, className }), children: asChild ? undefined : children }, childElement(children, asChild)); }
