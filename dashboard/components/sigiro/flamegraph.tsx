"use client";

import { select } from "d3-selection";
import flamegraph, { colorMapper, tooltip } from "d3-flame-graph";
import * as React from "react";

import { formatValue, percent, type FlameNode } from "@/lib/sigiro/profiles";

type Chart = ReturnType<typeof flamegraph>;

/**
 * d3-flame-graph (Apache-2.0) in a resizable box. Click a frame to zoom,
 * `search` highlights matching frames, and with `diff` frames are coloured by
 * `delta` (red grew, green shrank).
 */
export function Flamegraph({
  tree,
  unit,
  diff = false,
  search = "",
  resetKey,
}: {
  tree: FlameNode;
  unit: string;
  diff?: boolean;
  search?: string;
  /** Changing this resets zoom. */
  resetKey?: string;
}) {
  const box = React.useRef<HTMLDivElement>(null);
  const chart = React.useRef<Chart | null>(null);
  const [width, setWidth] = React.useState(0);

  React.useEffect(() => {
    const el = box.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) =>
      setWidth(Math.floor(entry.contentRect.width))
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  React.useEffect(() => {
    const el = box.current;
    if (!el || width === 0) return;
    const total = tree.value;
    const tip = tooltip
      .defaultFlamegraphTooltip()
      .html((d: { data: FlameNode; value: number; delta?: number }) => {
        const name = escapeHtml(d.data.name);
        const line = `${formatValue(d.value, unit)} · ${percent(d.value, total)}`;
        const change =
          diff && d.delta !== undefined
            ? `<br/>change: ${d.delta > 0 ? "+" : ""}${formatValue(d.delta, unit)}`
            : "";
        return `<div style="font-family:var(--font-mono);font-size:12px;max-width:640px;word-break:break-all">${name}</div><div style="font-size:12px;opacity:.8">${line}${change}</div>`;
      });
    const c = flamegraph()
      .width(width)
      .cellHeight(18)
      .minFrameSize(1)
      .transitionDuration(250)
      .sort(true)
      // Root on top, callees below (an icicle), as Pyroscope draws it.
      .inverted(true)
      .selfValue(false)
      .tooltip(tip)
      .label((d: { data: FlameNode; value: number }) => `${d.data.name} (${formatValue(d.value, unit)}, ${percent(d.value, total)})`);
    if (diff) c.computeDelta(true).setColorMapper(colorMapper.differentialColorMapper);
    el.replaceChildren();
    select(el).datum(tree).call(c);
    chart.current = c;
    return () => {
      c.destroy?.();
      document.querySelectorAll(".d3-flame-graph-tip").forEach((node) => node.remove());
      chart.current = null;
    };
  }, [tree, unit, diff, width, resetKey]);

  React.useEffect(() => {
    const c = chart.current;
    if (!c) return;
    if (search) c.search(search);
    else c.clear();
  }, [search, tree, width, diff]);

  return (
    <div
      ref={box}
      className="sigiro-flamegraph w-full overflow-x-hidden"
      data-flamegraph-total={tree.value}
      data-flamegraph-frames={countFrames(tree)}
    />
  );
}

function countFrames(node: FlameNode): number {
  return node.children.reduce((sum, child) => sum + countFrames(child), 1);
}

const escapeHtml = (s: string) =>
  s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
