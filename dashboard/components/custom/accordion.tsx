"use client";
import * as Sdk from "@ui-sdk/react/accordion";
import type * as React from "react";
export function Accordion({ type, ...props }: React.ComponentProps<typeof Sdk.Accordion> & { type?: "single" | "multiple" }) { return <Sdk.Accordion {...props} multiple={type === "multiple"} />; }
export const AccordionItem = Sdk.AccordionItem;
export const AccordionTrigger = Sdk.AccordionTrigger;
export const AccordionContent = Sdk.AccordionPanel;
