"use client";
import { createAgent } from "@ui-sdk/react/agent";
import { useEffect } from "react";

const agent = createAgent({ allow: () => false });
export function AgentBridge() {
  useEffect(() => {
    if (process.env.NODE_ENV !== "development") return;
    Object.assign(window, { sigiroUI: agent });
    return () => { Reflect.deleteProperty(window, "sigiroUI"); };
  }, []);
  return null;
}
