import { createLoader, type ParserBuilder } from "nuqs";
import SuperJSON from "superjson";

import type {
  FacetMetadataSchema,
  InfiniteQueryResponse,
} from "@/lib/data-table";
import type { Transport } from "@/lib/data-table/transport";

import { bucketFor, num, query, type Row, sqlString, ts } from "./query";

/**
 * The data-table-filters blocks expect an HTTP endpoint that filters, facets,
 * charts and cursor-paginates (their example ships an in-memory one at
 * `app/example/api/route.ts`). A static export has no server routes, so this
 * is that endpoint, run in the browser through the block's swappable
 * `transport.fetch`: the table's URL state becomes SQL for `/v1/query`.
 */
export type SqlSource<TRow> = {
  /** Table plus any fixed condition, e.g. root spans only. */
  from: string;
  base?: string;
  /** Column list for a page of rows; must include `timestamp`. */
  select: string;
  /** Series for the timeline chart and the `level` facet. */
  series: { key: string; sql: string; values: readonly string[] };
  /** Checkbox filters: URL key → SQL expression. */
  facets: Record<string, string>;
  /** Slider filters: URL key → numeric SQL expression. */
  sliders?: Record<string, string>;
  /** Input filters: URL key → text SQL expression searched with ILIKE. */
  inputs?: Record<string, string>;
  toRow: (row: Row) => TRow;
  /** The window used when the table has no date filter. */
  window: () => { from: Date; to: Date };
};

const inList = (sql: string, values: unknown[]) =>
  `${sql} IN (${values.map((value) => (typeof value === "number" ? String(value) : sqlString(String(value)))).join(", ")})`;

export function sqlTransport<TRow extends { timestamp: Date }>(
  source: SqlSource<TRow>,
  parsers: Record<string, ParserBuilder<unknown>>
): Transport<TRow[], unknown> {
  const load = createLoader(parsers);

  const handle = async (url: string, signal?: AbortSignal) => {
    const params = new URL(url, "https://sigiro.invalid").searchParams;
    const search = load(params) as Record<string, unknown>;
    const skipMeta = params.get("_meta") === "false";

    const dates = (search.timestamp as Date[] | null | undefined) ?? [];
    const fallback = source.window();
    const from = dates[0] ?? fallback.from;
    const to =
      dates[1] ??
      (dates[0] ? new Date(from.getTime() + 86_400_000) : fallback.to);

    const filters: string[] = [
      `timestamp >= ${ts(from)}`,
      `timestamp < ${ts(to)}`,
    ];
    if (source.base) filters.push(source.base);
    const seriesValues = search[source.series.key];
    if (Array.isArray(seriesValues) && seriesValues.length > 0)
      filters.push(inList(source.series.sql, seriesValues));
    for (const [key, sql] of Object.entries(source.facets)) {
      const values = search[key];
      if (Array.isArray(values) && values.length > 0)
        filters.push(inList(sql, values));
    }
    for (const [key, sql] of Object.entries(source.inputs ?? {})) {
      const value = search[key];
      if (typeof value === "string" && value.trim())
        filters.push(`${sql} ILIKE ${sqlString(`%${value.trim()}%`)}`);
    }
    const unslid = [...filters];
    for (const [key, sql] of Object.entries(source.sliders ?? {})) {
      const range = search[key];
      if (Array.isArray(range) && range.length > 0) {
        const [low, high] = range as number[];
        filters.push(`${sql} >= ${Number(low)}`);
        if (high !== undefined) filters.push(`${sql} <= ${Number(high)}`);
      }
    }
    const where = (clauses: string[]) =>
      `FROM ${source.from} WHERE ${clauses.join(" AND ")}`;

    const size = Number(search.size) > 0 ? Number(search.size) : 50;
    const cursor = (search.cursor as Date | null | undefined) ?? to;
    const page =
      search.direction === "prev"
        ? `SELECT ${source.select} ${where([...filters, `timestamp > ${ts(cursor)}`])} ORDER BY timestamp DESC LIMIT 500`
        : `SELECT ${source.select} ${where([...filters, `timestamp < ${ts(cursor)}`])} ORDER BY timestamp DESC LIMIT ${size}`;

    const facetKeys = [source.series.key, ...Object.keys(source.facets)];
    const facetSql = [source.series.sql, ...Object.values(source.facets)];
    const bucket = bucketFor(from, to);
    const sliderEntries = Object.entries(source.sliders ?? {});

    const [rows, facetRows, chartRows, totalRows, sliderRows] =
      await Promise.all([
        query(page, signal),
        skipMeta
          ? Promise.resolve([])
          : query(
              `SELECT ${facetSql.map((sql, index) => `${sql} AS f${index}`).join(", ")}, count(*) AS n ${where(filters)} GROUP BY ALL`,
              signal
            ),
        skipMeta
          ? Promise.resolve([])
          : query(
              `SELECT epoch_ms(time_bucket(INTERVAL '${bucket.sql}', timestamp)) AS t, ${source.series.sql} AS s, count(*) AS n ${where(filters)} GROUP BY ALL ORDER BY 1`,
              signal
            ),
        skipMeta
          ? Promise.resolve([])
          : query(
              `SELECT count(*) AS n ${where([`timestamp >= ${ts(from)}`, `timestamp < ${ts(to)}`, ...(source.base ? [source.base] : [])])}`,
              signal
            ),
        skipMeta || sliderEntries.length === 0
          ? Promise.resolve([])
          : query(
              `SELECT ${sliderEntries.map(([, sql], index) => `min(${sql}) AS lo${index}, max(${sql}) AS hi${index}`).join(", ")} ${where(unslid)}`,
              signal
            ),
      ]);

    const filterRowCount = facetRows.reduce((sum, row) => sum + num(row.n), 0);
    const facets: Record<string, FacetMetadataSchema> = {};
    facetKeys.forEach((key, index) => {
      const counts = new Map<string, number>();
      for (const row of facetRows) {
        const value = row[`f${index}`];
        if (value === null) continue;
        counts.set(
          String(value),
          (counts.get(String(value)) ?? 0) + num(row.n)
        );
      }
      facets[key] = {
        rows: [...counts].map(([value, total]) => ({ value, total })),
        total: filterRowCount,
      };
    });
    sliderEntries.forEach(([key], index) => {
      const row = sliderRows[0];
      facets[key] = {
        rows: [],
        total: filterRowCount,
        min: row ? Math.floor(num(row[`lo${index}`])) : undefined,
        max: row ? Math.ceil(num(row[`hi${index}`])) : undefined,
      };
    });

    const buckets = new Map<number, Record<string, number>>();
    for (
      let t = Math.floor(from.getTime() / bucket.ms) * bucket.ms;
      t < to.getTime();
      t += bucket.ms
    )
      buckets.set(
        t,
        Object.fromEntries(source.series.values.map((value) => [value, 0]))
      );
    for (const row of chartRows) {
      const point = buckets.get(num(row.t));
      if (point) point[String(row.s)] = num(row.n);
    }
    const chartData = [...buckets].map(([timestamp, counts]) => ({
      timestamp,
      ...counts,
    }));

    const data = rows.map(source.toRow);
    const response: InfiniteQueryResponse<TRow[]> = {
      data,
      meta: {
        totalRowCount: num(totalRows[0]?.n),
        filterRowCount,
        chartData,
        facets,
      },
      prevCursor: data.length > 0 ? data[0].timestamp.getTime() : null,
      nextCursor:
        data.length >= size ? data[data.length - 1].timestamp.getTime() : null,
    };
    return Response.json(SuperJSON.stringify(response));
  };

  return {
    baseUrl: "",
    fetch: ((input: RequestInfo | URL, init?: RequestInit) =>
      handle(String(input), init?.signal ?? undefined)) as typeof fetch,
  };
}
