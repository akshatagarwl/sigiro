import { num, sqlString, ts, type Row } from "./query";

/**
 * Profiles views: SQL over `sigiro_profiles` and the client-side shaping a
 * flame graph and the top-functions table need. The server groups samples by
 * stack, so the browser only ever sees one row per distinct stack.
 */

/** Sample types that are snapshots (a gauge), not counts over the window. */
const SNAPSHOT_TYPES = new Set(["inuse_objects", "inuse_space", "goroutines", "goroutine"]);
export const isSnapshot = (type: string) => SNAPSHOT_TYPES.has(type);

export type LabelFilter = { key: string; value: string };

export type ProfileQuery = {
  service: string;
  type: string;
  from: Date;
  to: Date;
  labels: LabelFilter[];
  traceId?: string;
  spanId?: string;
};

const jsonPath = (key: string) => sqlString(`$."${key.replaceAll('"', '\\"')}"`);

export function whereSql(q: ProfileQuery): string {
  const parts = [
    `service_name = ${sqlString(q.service)}`,
    `profile_type = ${sqlString(q.type)}`,
    `timestamp >= ${ts(q.from)}`,
    `timestamp < ${ts(q.to)}`,
  ];
  for (const label of q.labels)
    parts.push(`json_extract_string(labels, ${jsonPath(label.key)}) = ${sqlString(label.value)}`);
  if (q.traceId && /^[0-9a-f]{32}$/i.test(q.traceId))
    parts.push(`trace_id = ${sqlString(q.traceId.toLowerCase())}`);
  if (q.spanId && /^[0-9a-f]{16}$/i.test(q.spanId))
    parts.push(`span_id = ${sqlString(q.spanId.toLowerCase())}`);
  return parts.join(" AND ");
}

export const servicesSql = (from: Date, to: Date) =>
  `SELECT service_name AS s, count(*) AS n FROM sigiro_profiles WHERE timestamp >= ${ts(from)} AND timestamp < ${ts(to)} GROUP BY 1 ORDER BY 1`;

export const typesSql = (service: string, from: Date, to: Date) =>
  `SELECT profile_type AS type, any_value(unit) AS unit, count(*) AS n FROM sigiro_profiles WHERE service_name = ${sqlString(service)} AND timestamp >= ${ts(from)} AND timestamp < ${ts(to)} GROUP BY 1 ORDER BY 1`;

/** One row per distinct stack with its summed value, biggest first. */
export const stacksSql = (q: ProfileQuery) =>
  `SELECT stack_trace AS stack, sum(value) AS v FROM sigiro_profiles WHERE ${whereSql(q)} GROUP BY stack_trace ORDER BY v DESC LIMIT 50000`;

/** Distinct collection times, to average snapshot types over the window. */
export const summarySql = (q: ProfileQuery) =>
  `SELECT sum(value) AS total, count(DISTINCT timestamp) AS snapshots, count(*) AS rows, count(trace_id) AS linked FROM sigiro_profiles WHERE ${whereSql(q)}`;

export const timelineSql = (q: ProfileQuery, bucket: string) =>
  `SELECT epoch_us(time_bucket(INTERVAL '${bucket}', timestamp)) AS t, sum(value) AS v FROM sigiro_profiles WHERE ${whereSql(q)} GROUP BY 1 ORDER BY 1`;

/** String-valued sample labels in the window, by weight, for the filter picker. */
export const labelsSql = (q: ProfileQuery) => {
  const base = whereSql({ ...q, labels: [] });
  return `SELECT k AS key, json_extract_string(labels, '$."' || k || '"') AS val, sum(value) AS total FROM (SELECT labels, value, unnest(json_keys(labels)) AS k FROM sigiro_profiles WHERE ${base} AND labels IS NOT NULL) t WHERE json_type(labels, '$."' || k || '"') = 'VARCHAR' GROUP BY 1, 2 ORDER BY total DESC LIMIT 300`;
};

export type Stack = { frames: string[]; value: number };

export const toStacks = (rows: Row[] | undefined): Stack[] =>
  (rows ?? []).map((row) => ({
    frames: Array.isArray(row.stack) ? (row.stack as unknown[]).map(String) : [],
    value: num(row.v),
  }));

export type FlameNode = {
  name: string;
  value: number;
  delta?: number;
  children: FlameNode[];
};

type MutableNode = { name: string; value: number; delta: number; children: Map<string, MutableNode> };

const newNode = (name: string): MutableNode => ({ name, value: 0, delta: 0, children: new Map() });

function freeze(node: MutableNode, withDelta: boolean): FlameNode {
  return {
    name: node.name,
    value: node.value,
    ...(withDelta ? { delta: node.delta } : {}),
    children: [...node.children.values()].map((child) => freeze(child, withDelta)),
  };
}

/** A d3-flame-graph tree (values include children), root first. */
export function buildTree(stacks: Stack[], scale = 1, rootName = "total"): FlameNode {
  const root = newNode(rootName);
  for (const stack of stacks) {
    const v = stack.value * scale;
    root.value += v;
    let node = root;
    for (const frame of stack.frames) {
      let child = node.children.get(frame);
      if (!child) node.children.set(frame, (child = newNode(frame)));
      child.value += v;
      node = child;
    }
  }
  return freeze(root, false);
}

/**
 * A diff tree: each node is as wide as the larger of the comparison value and
 * the baseline scaled to the comparison's total (so removed paths still show),
 * and `delta` is comparison − scaled baseline (red grew, blue shrank).
 */
export function buildDiffTree(baseline: Stack[], comparison: Stack[], scale = 1): FlameNode {
  const totalA = baseline.reduce((s, x) => s + x.value, 0);
  const totalB = comparison.reduce((s, x) => s + x.value, 0);
  const norm = totalA > 0 ? totalB / totalA : 1;
  const a = new Map<string, number>();
  const b = new Map<string, number>();
  const add = (map: Map<string, number>, stacks: Stack[], factor: number) => {
    for (const stack of stacks) {
      let path = "";
      map.set("", (map.get("") ?? 0) + stack.value * factor);
      for (const frame of stack.frames) {
        path += "\u0000" + frame;
        map.set(path, (map.get(path) ?? 0) + stack.value * factor);
      }
    }
  };
  add(a, baseline, norm * scale);
  add(b, comparison, scale);
  const root = newNode("total");
  const nodes = new Map<string, MutableNode>([["", root]]);
  for (const path of new Set([...a.keys(), ...b.keys()])) {
    if (path === "") continue;
    const parts = path.split("\u0000").slice(1);
    let node = root;
    let key = "";
    for (const frame of parts) {
      key += "\u0000" + frame;
      let child = nodes.get(key);
      if (!child) {
        child = newNode(frame);
        nodes.set(key, child);
        node.children.set(frame, child);
      }
      node = child;
    }
  }
  for (const [key, node] of nodes) {
    const va = a.get(key) ?? 0;
    const vb = b.get(key) ?? 0;
    node.value = Math.max(va, vb);
    node.delta = vb - va;
  }
  // A node must be at least as wide as its children together.
  const fix = (node: MutableNode): number => {
    let sum = 0;
    for (const child of node.children.values()) sum += fix(child);
    node.value = Math.max(node.value, sum);
    return node.value;
  };
  fix(root);
  return freezeDiff(root);
}

function freezeDiff(node: MutableNode): FlameNode {
  // d3-flame-graph sums `delta` up the tree when computeDelta is on, so each
  // node carries only its self delta.
  let childDelta = 0;
  for (const child of node.children.values()) childDelta += child.delta;
  return {
    name: node.name,
    value: node.value,
    delta: node.delta - childDelta,
    children: [...node.children.values()].map(freezeDiff),
  };
}

export type FunctionRow = { name: string; self: number; total: number; selfB?: number; totalB?: number };

/** Self (leaf) and total (anywhere on the stack, once per stack) per function. */
export function topFunctions(stacks: Stack[], scale = 1): Map<string, { self: number; total: number }> {
  const out = new Map<string, { self: number; total: number }>();
  for (const stack of stacks) {
    const v = stack.value * scale;
    const seen = new Set<string>();
    stack.frames.forEach((frame, i) => {
      let entry = out.get(frame);
      if (!entry) out.set(frame, (entry = { self: 0, total: 0 }));
      if (!seen.has(frame)) {
        entry.total += v;
        seen.add(frame);
      }
      if (i === stack.frames.length - 1) entry.self += v;
    });
  }
  return out;
}

const UNITS: Record<string, { div: number; suffix: string }[]> = {
  nanoseconds: [
    { div: 3_600e9, suffix: "h" },
    { div: 60e9, suffix: "min" },
    { div: 1e9, suffix: "s" },
    { div: 1e6, suffix: "ms" },
    { div: 1e3, suffix: "µs" },
    { div: 1, suffix: "ns" },
  ],
  bytes: [
    { div: 1024 ** 4, suffix: "TiB" },
    { div: 1024 ** 3, suffix: "GiB" },
    { div: 1024 ** 2, suffix: "MiB" },
    { div: 1024, suffix: "KiB" },
    { div: 1, suffix: "B" },
  ],
};

/** A value in its unit, for labels and tables. */
export function formatValue(value: number, unit: string): string {
  const scales = UNITS[unit];
  const abs = Math.abs(value);
  if (abs < 1e-9) return "0";
  if (!scales) {
    if (abs >= 1e9) return `${(value / 1e9).toFixed(2)}G`;
    if (abs >= 1e6) return `${(value / 1e6).toFixed(2)}M`;
    if (abs >= 1e4) return `${(value / 1e3).toFixed(1)}k`;
    return Number.isInteger(value) ? String(value) : value.toFixed(1);
  }
  const s = scales.find((x) => abs >= x.div) ?? scales[scales.length - 1];
  const scaled = value / s.div;
  return `${Math.abs(scaled) >= 100 ? scaled.toFixed(0) : scaled.toFixed(2)} ${s.suffix}`;
}

export const percent = (part: number, whole: number) =>
  whole > 0 ? `${((100 * part) / whole).toFixed(2)}%` : "–";

/** Human labels for the profile types Go profilers send. */
export const TYPE_LABELS: Record<string, string> = {
  cpu: "CPU time",
  samples: "CPU samples",
  alloc_space: "Allocated bytes",
  alloc_objects: "Allocated objects",
  inuse_space: "In-use bytes",
  inuse_objects: "In-use objects",
  goroutines: "Goroutines",
  mutex_count: "Mutex contentions",
  mutex_duration: "Mutex wait time",
  block_count: "Block events",
  block_duration: "Block time",
};
