import { useEffect, useRef, useState } from "react";
import uPlot from "uplot";

export type Series = { label: string; values: number[]; tone: "ink" | "flag" };

type Props = {
  label: string;
  times: number[];
  series: Series[];
  selection: [number, number] | null;
  onSelect: (range: [number, number] | null) => void;
};

const token = (name: string) =>
  getComputedStyle(document.documentElement).getPropertyValue(name).trim();

const HEIGHT = 168;

export function Chart({ label, times, series, selection, onSelect }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const [revision, setRevision] = useState(0);
  const select = useRef(onSelect);
  useEffect(() => {
    select.current = onSelect;
  });

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    const build = () => {
      plot.current?.destroy();
      const ink3 = token("--ink-3");
      const hairline = token("--hairline");
      const axis = {
        stroke: ink3,
        grid: { stroke: hairline, width: 1 },
        ticks: { stroke: hairline, width: 1 },
        font: "10.5px ui-monospace, monospace",
      };
      plot.current = new uPlot(
        {
          width: element.clientWidth,
          height: HEIGHT,
          legend: { show: true, live: true },
          cursor: { drag: { x: true, y: false, setScale: false } },
          tzDate: (seconds: number) =>
            uPlot.tzDate(new Date(seconds * 1000), "Etc/UTC"),
          scales: { x: { time: true } },
          axes: [axis, { ...axis, size: 48 }],
          series: [
            { label: "UTC", value: "{YYYY}-{MM}-{DD} {HH}:{mm}" },
            ...series.map((entry) => ({
              label: entry.label,
              stroke: token(entry.tone === "flag" ? "--flag" : "--ink-2"),
              width: 1.5,
              points: { show: false },
            })),
          ],
          hooks: {
            setSelect: [
              (u: uPlot) => {
                if (u.select.width < 2) return;
                select.current([
                  u.posToVal(u.select.left, "x"),
                  u.posToVal(u.select.left + u.select.width, "x"),
                ]);
                u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
              },
            ],
          },
        },
        [times, ...series.map((entry) => entry.values)],
        element
      );
      setRevision((value) => value + 1);
    };
    build();
    const resize = new ResizeObserver(() =>
      plot.current?.setSize({ width: element.clientWidth, height: HEIGHT })
    );
    resize.observe(element);
    const theme = new MutationObserver(build);
    theme.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => {
      resize.disconnect();
      theme.disconnect();
      plot.current?.destroy();
      plot.current = null;
    };
  }, [times, series]);

  useEffect(() => {
    const u = plot.current;
    if (!u) return;
    let band = u.over.querySelector<HTMLDivElement>("[data-selection]");
    if (!selection) {
      band?.remove();
      return;
    }
    if (!band) {
      band = document.createElement("div");
      band.dataset.selection = "";
      band.className = "pointer-events-none absolute top-0 h-full bg-accent/10";
      u.over.appendChild(band);
    }
    const left = Math.max(0, u.valToPos(selection[0], "x"));
    const right = Math.min(u.over.clientWidth, u.valToPos(selection[1], "x"));
    band.style.left = `${left}px`;
    band.style.width = `${Math.max(0, right - left)}px`;
  }, [selection, revision]);

  const totals = series
    .map(
      (entry) =>
        `${entry.label} ${entry.values.reduce((a, b) => a + b, 0).toLocaleString("en")}`
    )
    .join(", ");

  return (
    <figure className="m-0 min-w-0" aria-label={`${label}: ${totals}`}>
      <div
        ref={host}
        className="[&_.u-legend]:text-ink-3 min-h-[168px] [&_.u-legend]:font-mono [&_.u-legend]:text-[10.5px]"
      />
    </figure>
  );
}
