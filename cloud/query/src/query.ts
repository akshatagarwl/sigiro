import { WorkerEntrypoint } from "cloudflare:workers";
import { DuckDB, init, type Connection } from "@ducklings/workers";
import wasmModule from "../vendor/duckdb-workers.wasm";

export type ViewSource = { keys: string[]; empty: Uint8Array };
export type QueryPlan = { sql: string; maxRows: number; views: Record<string, ViewSource> };
export type QueryOutcome =
  | { ok: true; rows: Record<string, unknown>[]; truncated: boolean }
  | { ok: false; kind: "memory" | "scan_limit" | "execution"; detail: string };

type Env = { SEGMENTS: R2Bucket; QUERY_MAX_SCAN_BYTES?: string; QUERY_MEMORY_LIMIT?: string };

const files = new Map<string, Uint8Array<ArrayBuffer>>();
const nativeFetch = globalThis.fetch.bind(globalThis);
let database: DuckDB | undefined;

globalThis.fetch = async (input: RequestInfo | URL, options?: RequestInit) => {
  const request = new Request(input, options);
  if (!request.url.endsWith(".parquet") || !new URL(request.url).hostname.endsWith(".segments.invalid")) {
    return nativeFetch(request);
  }
  const bytes = files.get(request.url);
  if (bytes === undefined) {
    return new Response(null, { status: 404 });
  }
  const headers = { "accept-ranges": "bytes", "content-length": String(bytes.byteLength) };
  if (request.method === "HEAD") {
    return new Response(null, { headers });
  }
  const range = /^bytes=(\d+)-(\d*)$/u.exec(request.headers.get("range") ?? "");
  if (range === null) {
    return new Response(bytes, { headers });
  }
  const start = Number(range[1]);
  const end = range[2] === "" ? bytes.byteLength - 1 : Math.min(Number(range[2]), bytes.byteLength - 1);
  const slice = bytes.subarray(start, end + 1);
  return new Response(slice, {
    status: 206,
    headers: { "accept-ranges": "bytes", "content-length": String(slice.byteLength), "content-range": `bytes ${start}-${end}/${bytes.byteLength}` },
  });
};

export class Query extends WorkerEntrypoint<Env> {
  async run(plan: QueryPlan): Promise<QueryOutcome> {
    const host = `https://q${crypto.randomUUID().replaceAll("-", "")}.segments.invalid`;
    const urls: string[] = [];
    try {
      const sources = await this.prefetch(plan.views, host, urls);
      if (!sources.ok) {
        return sources;
      }
      const connection = (await this.database()).connect();
      try {
        for (const [view, paths] of Object.entries(sources.views)) {
          const list = paths.map((path) => `'${path}'`).join(", ");
          await connection.execute(`CREATE TEMP VIEW ${view} AS SELECT * FROM read_parquet([${list}])`);
        }
        return await runQuery(connection, plan);
      } finally {
        connection.close();
      }
    } catch (error) {
      return classify(error);
    } finally {
      for (const url of urls) {
        files.delete(url);
      }
    }
  }

  private async database(): Promise<DuckDB> {
    if (database === undefined) {
      await init({ wasmModule });
      database = new DuckDB({ customConfig: { memory_limit: this.env.QUERY_MEMORY_LIMIT ?? "64MB", threads: "1" } });
    }
    return database;
  }

  private async prefetch(
    views: Record<string, ViewSource>,
    host: string,
    urls: string[],
  ): Promise<{ ok: true; views: Record<string, string[]> } | { ok: false; kind: "scan_limit"; detail: string }> {
    const limit = Number(this.env.QUERY_MAX_SCAN_BYTES ?? 25_165_824);
    const keys = [...new Set(Object.values(views).flatMap((source) => source.keys))];
    const objects = await Promise.all(keys.map(async (key) => [key, await this.env.SEGMENTS.get(key)] as const));
    const scanned = objects.reduce((total, [, object]) => total + (object?.size ?? 0), 0);
    if (scanned > limit) {
      return { ok: false, kind: "scan_limit", detail: `query would scan ${scanned} bytes of segments, above the ${limit}-byte limit; narrow it to fewer tables` };
    }
    const byKey = new Map<string, string>();
    for (const [index, [key, object]] of objects.entries()) {
      if (object !== null) {
        const url = `${host}/${index}.parquet`;
        files.set(url, new Uint8Array(await object.arrayBuffer()));
        urls.push(url);
        byKey.set(key, url);
      }
    }
    const resolved: Record<string, string[]> = {};
    for (const [view, source] of Object.entries(views)) {
      const empty = `${host}/${view}.parquet`;
      files.set(empty, new Uint8Array(source.empty));
      urls.push(empty);
      resolved[view] = [empty, ...source.keys.flatMap((key) => byKey.get(key) ?? [])];
    }
    return { ok: true, views: resolved };
  }
}

async function runQuery(connection: Connection, plan: QueryPlan): Promise<QueryOutcome> {
  const rows = await connection.query(plan.sql);
  const truncated = rows.length > plan.maxRows;
  return { ok: true, rows: (truncated ? rows.slice(0, plan.maxRows) : rows).map(toJsonRow), truncated };
}

function toJsonRow(row: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(row).map(([column, value]) => [column, toJsonValue(value)]));
}

function toJsonValue(value: unknown): unknown {
  if (typeof value === "bigint") {
    return Number.isSafeInteger(Number(value)) ? Number(value) : value.toString();
  }
  if (value instanceof Date) {
    return value.toISOString();
  }
  return value;
}

function classify(error: unknown): QueryOutcome {
  const detail = error instanceof Error ? error.message : String(error);
  if (/out of memory/iu.test(detail)) {
    return { ok: false, kind: "memory", detail: `query exceeded the query memory limit: ${detail}` };
  }
  return { ok: false, kind: "execution", detail };
}

export default {
  fetch(): Response {
    return new Response("not found", { status: 404 });
  },
};
