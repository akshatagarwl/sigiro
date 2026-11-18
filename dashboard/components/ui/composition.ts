import { isValidElement } from "react";
export function childElement(child: unknown, asChild?: boolean) {
  return asChild && isValidElement<Record<string, unknown>>(child) ? child : undefined;
}
