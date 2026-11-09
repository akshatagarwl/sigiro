import * as React from "react";
import { createRoot } from "react-dom/client";

import { EventExplorer } from "../../components/observability/event-explorer";
import type { EventRecord } from "../../components/observability/types";

const shared = { preserved: "repeated reference, not a cycle" };
const graph: Record<string, unknown> = {
  integer: BigInt("123456789012345678901234567890"),
  oversized: "synthetic graph payload ".repeat(50_000),
  first: shared,
  second: shared,
  wide: Object.fromEntries(
    Array.from({ length: 200 }, (_, i) => [
      `branch${i}`,
      Object.fromEntries(Array.from({ length: 200 }, (_, j) => [`key${j}`, j])),
    ])
  ),
};
graph.self = graph;
let deep: Record<string, unknown> = {};
if (window.location.search.includes("deep=1")) {
  for (let i = 0; i < 20_000; i++) deep = { child: deep };
}
const event: EventRecord = {
  id: "explicit-synthetic-graph",
  timestamp: new Date("2026-11-09T12:00:00Z"),
  service: "synthetic-component-fixture",
  severity: "INFO",
  category: "info",
  message: "synthetic oversized message ".repeat(50_000),
  attributes: window.location.search.includes("deep=1") ? deep : graph,
};

function Fixture() {
  const [id, setId] = React.useState<string>();
  return (
    <>
      <h1>Synthetic standalone component fixture — not production telemetry</h1>
      <EventExplorer events={[event]} selectedId={id} onSelect={setId} />
    </>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
