"use client";
import * as Sdk from "@ui-sdk/react/tooltip";
import * as React from "react";
export function TooltipProvider({ children }: { children: React.ReactNode; delayDuration?: number }) { return children; }
export function Tooltip({ delayDuration, ...props }: React.ComponentProps<typeof Sdk.Tooltip> & { delayDuration?: number }) { return <Sdk.Tooltip {...props} delay={delayDuration} />; }
export function TooltipTrigger({ asChild, children, ...props }: React.ComponentProps<typeof Sdk.TooltipTrigger> & { asChild?: boolean }) {
  const render = asChild && React.isValidElement<Record<string, unknown>>(children) ? children : undefined;
  return <Sdk.TooltipTrigger {...props} render={render}>{render ? undefined : children}</Sdk.TooltipTrigger>;
}
export const TooltipContent = Sdk.TooltipContent;
