"use client";

import * as Sdk from "@ui-sdk/react/select";
import type * as React from "react";

export function Select({ onValueChange, ...props }: Omit<Sdk.SelectProps<string, false>, "onValueChange" | "multiple" | "value" | "defaultValue"> & { value?: string | null; defaultValue?: string | null; onValueChange?: (value: string) => void }) {
  return <Sdk.Select<string, false> {...props} multiple={false} onValueChange={(value) => { if (value !== null) onValueChange?.(value); }} />;
}
export function SelectTrigger({ size: _size, ...props }: React.ComponentProps<typeof Sdk.SelectTrigger> & { size?: "sm" | "default" }) {
  return <Sdk.SelectTrigger {...props} />;
}
export function SelectContent({ position: _position, ...props }: React.ComponentProps<typeof Sdk.SelectContent> & { position?: "item-aligned" | "popper" }) {
  return <Sdk.SelectContent {...props} />;
}
export const SelectValue = Sdk.SelectValue;
export const SelectItem = Sdk.SelectItem;
export const SelectGroup = Sdk.SelectGroup;
export const SelectLabel = Sdk.SelectGroupLabel;
export const SelectSeparator = Sdk.SelectSeparator;
