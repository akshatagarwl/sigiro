"use client";
import * as Sdk from "@ui-sdk/react/tabs";
import type * as React from "react";
export function Tabs({ onValueChange, ...props }: Omit<React.ComponentProps<typeof Sdk.Tabs>, "onValueChange"> & { onValueChange?: (value: string) => void }) {
  return <Sdk.Tabs {...props} onValueChange={(value) => { if (value !== null) onValueChange?.(value); }} />;
}
export const TabsList = Sdk.TabsList;
export const TabsTrigger = Sdk.TabsTab;
export const TabsContent = Sdk.TabsPanel;
