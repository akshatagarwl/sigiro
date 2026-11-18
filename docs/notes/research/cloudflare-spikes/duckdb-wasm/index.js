import { init, DuckDB } from "@ducklings/workers";

import wasmModule from "./duckdb-workers.wasm";

const SEGMENTS = "https://segments.invalid/";
let conn;
let bucket;
const nativeFetch = globalThis.fetch.bind(globalThis);
const stats = { gets: 0, heads: 0, bytes: 0 };

globalThis.fetch = async (input, initOpts) => {
  const request = new Request(input, initOpts);
  if (!request.url.startsWith(SEGMENTS)) return nativeFetch(request);
  const key = decodeURIComponent(request.url.slice(SEGMENTS.length));
  if (request.method === "HEAD") {
    stats.heads++;
    const head = await bucket.head(key);
    if (!head) return new Response(null, { status: 404 });
    return new Response(null, {
      headers: {
        "content-length": String(head.size),
        "accept-ranges": "bytes",
        "last-modified": head.uploaded.toUTCString(),
        etag: head.httpEtag,
      },
    });
  }
  const range = request.headers.get("range");
  let options = {};
  if (range) {
    const m = /bytes=(\d+)-(\d*)/.exec(range);
    const offset = Number(m[1]);
    options.range = m[2]
      ? { offset, length: Number(m[2]) - offset + 1 }
      : { offset };
  }
  stats.gets++;
  const object = await bucket.get(key, options);
  if (!object) return new Response(null, { status: 404 });
  const body = await object.arrayBuffer();
  stats.bytes += body.byteLength;
  const headers = {
    "content-length": String(body.byteLength),
    "accept-ranges": "bytes",
    etag: object.httpEtag,
    "last-modified": object.uploaded.toUTCString(),
  };
  if (range) {
    const start = options.range.offset;
    headers["content-range"] =
      `bytes ${start}-${start + body.byteLength - 1}/${object.size}`;
    return new Response(body, { status: 206, headers });
  }
  return new Response(body, { headers });
};

export default {
  async fetch(request, env) {
    bucket = env.SEGMENTS;
    const url = new URL(request.url);
    if (url.pathname === "/r2lat") {
      const size = Number(url.searchParams.get("size") ?? 131072);
      const out = [];
      for (let i = 0; i < 10; i++) {
        const t = performance.now();
        const o = await env.SEGMENTS.get("spans.parquet", {
          range: { offset: i * 4000000, length: size },
        });
        await o.arrayBuffer();
        out.push(Math.round(performance.now() - t));
      }
      const t = performance.now();
      await Promise.all(
        Array.from({ length: 10 }, (_, i) =>
          env.SEGMENTS.get("spans.parquet", {
            range: { offset: i * 4000000 + 7, length: size },
          }).then((o) => o.arrayBuffer())
        )
      );
      return Response.json({
        sequentialMs: out,
        parallel10Ms: Math.round(performance.now() - t),
      });
    }
    if (url.pathname === "/put") {
      await env.SEGMENTS.put(url.searchParams.get("key"), request.body);
      return new Response("ok");
    }
    const t0 = Date.now();
    if (!conn) {
      await init({ wasmModule });
      conn = new DuckDB().connect();
    }
    const t1 = Date.now();
    Object.assign(stats, { gets: 0, heads: 0, bytes: 0 });
    const sql =
      url.searchParams.get("sql") ??
      `SELECT service_name, count(*) AS n, avg(duration_us)::BIGINT AS avg_us, sum((status_code=2)::INT)::BIGINT AS errors FROM read_parquet('${SEGMENTS}spans.parquet') WHERE service_name IN ('svc-3','svc-7') GROUP BY 1 ORDER BY 1`;
    try {
      const rows = await conn.query(sql);
      return new Response(
        JSON.stringify(
          { rows, initMs: t1 - t0, queryMs: Date.now() - t1, stats },
          (_, v) => (typeof v === "bigint" ? Number(v) : v)
        ),
        { headers: { "content-type": "application/json" } }
      );
    } catch (error) {
      return Response.json({ error: String(error), stats }, { status: 500 });
    }
  },
};
