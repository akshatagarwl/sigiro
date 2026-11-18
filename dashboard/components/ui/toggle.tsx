"use client";
import { Toggle as SdkToggle } from "@ui-sdk/react/toggle";
import type * as React from "react";
import { buttonVariants } from "./button";
export const toggleVariants = buttonVariants;
export function Toggle({ variant = "outline", size = "sm", className, ...props }: Omit<React.ComponentProps<typeof SdkToggle>, "className"> & { variant?: "default" | "outline"; size?: "default" | "sm" | "lg"; className?: string }) {
  return <SdkToggle {...props} className={buttonVariants({ variant, size, className })} />;
}
