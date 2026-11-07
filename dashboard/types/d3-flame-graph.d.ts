// d3-flame-graph ships no types; this covers the API the profiles page uses.
declare module "d3-flame-graph" {
  type Node = any; // eslint-disable-line @typescript-eslint/no-explicit-any
  export interface FlameGraph {
    (selection: unknown): void;
    width(px: number): FlameGraph;
    cellHeight(px: number): FlameGraph;
    minFrameSize(px: number): FlameGraph;
    transitionDuration(ms: number): FlameGraph;
    sort(enabled: boolean): FlameGraph;
    selfValue(enabled: boolean): FlameGraph;
    inverted(enabled: boolean): FlameGraph;
    computeDelta(enabled: boolean): FlameGraph;
    tooltip(tip: unknown): FlameGraph;
    label(fn: (d: Node) => string): FlameGraph;
    setColorMapper(fn: (d: Node, originalColor: string) => string): FlameGraph;
    search(term: string): void;
    clear(): void;
    resetZoom(): void;
    update(data?: unknown): void;
    destroy?(): void;
  }
  export default function flamegraph(): FlameGraph;
  export const colorMapper: {
    differentialColorMapper: (d: Node, originalColor: string) => string;
    allocationColorMapper: (d: Node, originalColor: string) => string;
    offCpuColorMapper: (d: Node, originalColor: string) => string;
  };
  export const tooltip: {
    defaultFlamegraphTooltip(): {
      html(fn: (d: Node) => string): unknown;
      text(fn: (d: Node) => string): unknown;
    };
  };
}
