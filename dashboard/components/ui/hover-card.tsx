"use client";
import * as Sdk from "@ui-sdk/react/hover-card";
import * as React from "react";
export function HoverCard({ openDelay, closeDelay, ...props }: React.ComponentProps<typeof Sdk.HoverCard> & { openDelay?: number; closeDelay?: number }) { return <Sdk.HoverCard {...props} delay={openDelay} closeDelay={closeDelay} />; }
export function HoverCardTrigger({ asChild, children, ...props }: React.ComponentProps<typeof Sdk.HoverCardTrigger> & { asChild?: boolean }) {
  const render = asChild && React.isValidElement<Record<string, unknown>>(children) ? children : undefined;
  return <Sdk.HoverCardTrigger {...props} render={render}>{render ? undefined : children}</Sdk.HoverCardTrigger>;
}
export const HoverCardContent = Sdk.HoverCardContent;
