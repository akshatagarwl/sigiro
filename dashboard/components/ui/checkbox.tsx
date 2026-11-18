"use client";
import { Checkbox as SdkCheckbox, CheckboxIndicator } from "@ui-sdk/react/checkbox";
import type * as React from "react";
export function Checkbox(props: React.ComponentProps<typeof SdkCheckbox>) { return <SdkCheckbox {...props}><CheckboxIndicator /></SdkCheckbox>; }
