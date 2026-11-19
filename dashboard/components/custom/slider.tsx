"use client";
import * as Sdk from "@ui-sdk/react/slider";
import type * as React from "react";
export function Slider({ value, defaultValue, onValueChange, ...props }: Omit<React.ComponentProps<typeof Sdk.Slider>, "value" | "defaultValue" | "onValueChange"> & { value?: number[]; defaultValue?: number[]; onValueChange?: (value: number[]) => void }) {
  return <Sdk.Slider aria-label="Filter range" {...props} value={value} defaultValue={defaultValue} onValueChange={(next) => onValueChange?.(Array.isArray(next) ? next : [next])}><Sdk.SliderControl><Sdk.SliderTrack><Sdk.SliderIndicator /></Sdk.SliderTrack>{(value ?? defaultValue ?? [0]).map((_, index) => <Sdk.SliderThumb key={index} index={index} aria-label={index === 0 ? "Minimum" : "Maximum"} />)}</Sdk.SliderControl></Sdk.Slider>;
}
