import { createPublicKey, verify as verifySignature } from "node:crypto";

import { DurableObject } from "cloudflare:workers";

import type { Query, QueryOutcome, ViewSource } from "../query/src/query.ts";
import initWasm, {
  decode_otlp,
  empty_segment,
  manifest_commit,
  manifest_empty,
  manifest_live,
  manifest_register,
  validate_read_only,
} from "../vendor/segments-wasm/sigiro_segments_wasm.js";
import { reconcile, type Schema, type SchemaStore } from "./schema";

const ingestCellSchema: Schema = {
  wal: "CREATE TABLE {table} (id INTEGER PRIMARY KEY AUTOINCREMENT, tenant TEXT NOT NULL, signal TEXT NOT NULL, format TEXT NOT NULL, sdk_name TEXT, body BLOB NOT NULL, bytes INTEGER NOT NULL)",
  manifest:
    "CREATE TABLE {table} (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL)",
};

function sqlStore(storage: DurableObjectStorage): SchemaStore {
  return {
    all: (sql, ...params) =>
      storage.sql.exec(sql, ...params).toArray() as Record<string, unknown>[],
    run: (sql, ...params) => void storage.sql.exec(sql, ...params),
    transaction: (fn) => storage.transactionSync(fn),
  };
}
import wasmModule from "../vendor/segments-wasm/sigiro_segments_wasm_bg.wasm";

await initWasm(wasmModule);

type Signal = "traces" | "logs" | "metrics" | "profiles";
type Format = "protobuf" | "json";
type Table =
  | "spans"
  | "logs"
  | "metrics_gauge"
  | "metrics_sum"
  | "metrics_histogram"
  | "metrics_exp_histogram"
  | "profiles";

type Env = {
  INGEST_CELL: DurableObjectNamespace<IngestCell>;
  QUERY: Service<Query>;
  SIGIRO_MAX_RESULT_ROWS?: string;
  SEGMENTS: R2Bucket;
  SIGIRO_AUTH_ISSUER: string;
  SIGIRO_AUTH_AUDIENCE: string;
  SIGIRO_AUTH_JWKS_URL: string;
  SIGIRO_MAX_INGEST_BODY_BYTES?: string;
  FLUSH_SECONDS?: string;
  FLUSH_BYTES?: string;
};

type TenantId = string & { readonly TenantId: unique symbol };

type Claims = {
  iss: string;
  aud: string | string[];
  exp: number;
  nbf?: number;
  sub: string;
  tid: string;
  scope: string;
};

type Jwk = JsonWebKey & { kid?: string; alg?: string };
type Jwks = { keys: Jwk[] };
type WalRow = {
  id: number;
  tenant: TenantId;
  signal: Signal;
  format: string;
  body: ArrayBuffer;
  bytes: number;
};
type SegmentMetadata = {
  table: Table;
  min_time_us: number;
  max_time_us: number;
  rows: number;
  bytes: number;
};
type ManifestShape = {
  pending: Record<string, Table>;
  live: Record<string, { table: Table }>;
};

const ingestScope = "telemetry:ingest";
const queryScope = "query:read";
const servicesScope = "services:read";
const defaultMaxResultRows = 10_000;
const views: Record<string, { table: Table; signal: Signal }> = {
  sigiro_spans: { table: "spans", signal: "traces" },
  sigiro_logs: { table: "logs", signal: "logs" },
  sigiro_metrics_gauge: { table: "metrics_gauge", signal: "metrics" },
  sigiro_metrics_sum: { table: "metrics_sum", signal: "metrics" },
  sigiro_metrics_histogram: { table: "metrics_histogram", signal: "metrics" },
  sigiro_metrics_exp_histogram: {
    table: "metrics_exp_histogram",
    signal: "metrics",
  },
  sigiro_profiles: { table: "profiles", signal: "profiles" },
};
const servicesSql = `SELECT service_name, min(first_seen)::VARCHAR AS first_seen, max(last_seen)::VARCHAR AS last_seen,
  sum(span_count)::BIGINT AS span_count, sum(log_count)::BIGINT AS log_count, sum(metric_count)::BIGINT AS metric_count,
  sum(profile_count)::BIGINT AS profile_count, sum(span_count + log_count + metric_count + profile_count)::BIGINT AS total_count
FROM (
  SELECT service_name, min(timestamp) AS first_seen, max(timestamp) AS last_seen, count(*) AS span_count, 0::BIGINT AS log_count, 0::BIGINT AS metric_count, 0::BIGINT AS profile_count FROM sigiro_spans GROUP BY service_name
  UNION ALL SELECT service_name, min(timestamp), max(timestamp), 0, count(*), 0, 0 FROM sigiro_logs GROUP BY service_name
  UNION ALL SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*), 0 FROM sigiro_metrics_gauge GROUP BY service_name
  UNION ALL SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*), 0 FROM sigiro_metrics_sum GROUP BY service_name
  UNION ALL SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*), 0 FROM sigiro_metrics_histogram GROUP BY service_name
  UNION ALL SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*), 0 FROM sigiro_metrics_exp_histogram GROUP BY service_name
  UNION ALL SELECT service_name, min(timestamp), max(timestamp), 0, 0, 0, count(*) FROM sigiro_profiles GROUP BY service_name
) GROUP BY service_name ORDER BY total_count DESC, service_name ASC`;
const defaultBodyLimit = 8_388_608;
const defaultFlushSeconds = 60;
const defaultFlushBytes = 4_194_304;

let jwksCache: { url: string; keys: Jwk[]; expiresAt: number } | undefined;

export class IngestCell extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    reconcile(sqlStore(ctx.storage), ingestCellSchema);
  }

  async fetch(request: Request): Promise<Response> {
    if (request.method !== "POST") {
      return new Response("method not allowed", { status: 405 });
    }
    const params = new URL(request.url).searchParams;
    const signal = parseSignal(params.get("signal"));
    const tenant = parseTenantId(params.get("tenant"));
    const format = parseCellFormat(params.get("format"));
    if (signal === undefined || format === undefined || tenant === undefined) {
      return new Response("bad ingest cell request", { status: 400 });
    }
    const body = await request.arrayBuffer();
    try {
      decode_otlp(signal, format, new Uint8Array(body));
    } catch (error) {
      return new Response(String(error), { status: 400 });
    }
    this.ctx.storage.sql.exec(
      "INSERT INTO wal (tenant, signal, format, body, bytes) VALUES (?, ?, ?, ?, ?)",
      tenant,
      signal,
      format,
      body,
      body.byteLength
    );
    await this.ctx.storage.sync?.();
    await this.scheduleIfNeeded();
    return new Response(null, { status: 202 });
  }

  liveKeys(table: Table): string[] {
    const segments = JSON.parse(
      manifest_live(
        readManifest(this.ctx.storage.sql),
        table,
        -(2n ** 63n),
        2n ** 63n - 1n
      )
    ) as { key: string }[];
    return segments.map((segment) => segment.key);
  }

  async alarm(): Promise<void> {
    await this.flush();
  }

  private async scheduleIfNeeded(): Promise<void> {
    const total = sumWalBytes(this.ctx.storage.sql);
    if (total >= numberVar(this.env.FLUSH_BYTES, defaultFlushBytes)) {
      await this.flush();
      return;
    }
    const existing = await this.ctx.storage.getAlarm();
    if (existing === null) {
      await this.ctx.storage.setAlarm(
        Date.now() +
          numberVar(this.env.FLUSH_SECONDS, defaultFlushSeconds) * 1000
      );
    }
  }

  private async flush(): Promise<void> {
    const rows = this.ctx.storage.sql
      .exec<WalRow>(
        "SELECT id, tenant, signal, format, body, bytes FROM wal ORDER BY id"
      )
      .toArray();
    if (rows.length === 0) {
      return;
    }
    let manifest = readManifest(this.ctx.storage.sql);
    for (const row of rows) {
      const format = parseCellFormat(row.format);
      if (format === undefined) {
        console.warn(
          `dropping wal row ${row.id} in removed format ${row.format}`
        );
        this.ctx.storage.sql.exec("DELETE FROM wal WHERE id = ?", row.id);
        await this.ctx.storage.sync?.();
        continue;
      }
      const outputs = decode_otlp(row.signal, format, new Uint8Array(row.body));
      for (const [index, output] of outputs.entries()) {
        const metadata = JSON.parse(output.metadata) as SegmentMetadata;
        const key = objectKey(row.tenant, row.id, index, metadata.table);
        manifest = registerPending(manifest, key, metadata.table);
        writeManifest(this.ctx.storage.sql, manifest);
        await this.ctx.storage.sync?.();
        await this.env.SEGMENTS.put(key, output.parquet);
        manifest = commitLive(manifest, key, metadata);
        writeManifest(this.ctx.storage.sql, manifest);
        await this.ctx.storage.sync?.();
      }
      this.ctx.storage.sql.exec("DELETE FROM wal WHERE id = ?", row.id);
      writeManifest(this.ctx.storage.sql, manifest);
      await this.ctx.storage.sync?.();
    }
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "POST" && url.pathname === "/v1/query") {
      return queryRoute(request, env);
    }
    if (request.method === "GET" && url.pathname === "/v1/services") {
      return servicesRoute(request, env);
    }
    const route = routeSignal(url.pathname);
    if (request.method !== "POST" || route === undefined) {
      return new Response("not found", { status: 404 });
    }
    const limit = numberVar(env.SIGIRO_MAX_INGEST_BODY_BYTES, defaultBodyLimit);
    const length = request.headers.get("content-length");
    if (length !== null && Number(length) > limit) {
      return new Response("request body exceeds ingest limit", { status: 413 });
    }
    const contentType = mediaType(request.headers.get("content-type"));
    const format = parseFormat(contentType);
    if (format === undefined) {
      return new Response("unsupported content type", { status: 415 });
    }
    const auth = await authenticate(request, env, ingestScope);
    if (auth.status !== 200) {
      return new Response(auth.body, { status: auth.status });
    }
    const decoded = await decodedBody(request, limit);
    if (decoded.status !== 200) {
      return new Response(decoded.detail, { status: decoded.status });
    }
    const id = env.INGEST_CELL.idFromName(cellName(auth.tenant, route));
    const stub = env.INGEST_CELL.get(id);
    const params = new URLSearchParams({
      signal: route,
      tenant: auth.tenant,
      format,
    });
    try {
      return await stub.fetch(`https://cell.invalid/ingest?${params}`, {
        method: "POST",
        body: decoded.body,
      });
    } catch (error) {
      return new Response(`ingest cell unavailable: ${String(error)}`, {
        status: 503,
        headers: { "retry-after": "1" },
      });
    }
  },
};

async function queryRoute(request: Request, env: Env): Promise<Response> {
  const auth = await authenticate(request, env, queryScope);
  if (auth.status !== 200) {
    return problem(
      auth.status,
      auth.status === 401 ? "unauthorized" : "forbidden",
      auth.body
    );
  }
  const sql = await request.text();
  try {
    validate_read_only(sql);
  } catch (error) {
    return problem(400, "query rejected", String(error));
  }
  const outcome = await runQuery(env, auth.tenant, sql, referencedViews(sql));
  if (!outcome.ok) {
    return problem(400, "query rejected", outcome.detail);
  }
  return Response.json(outcome.rows, {
    headers: { "x-sigiro-truncated": String(outcome.truncated) },
  });
}

async function servicesRoute(request: Request, env: Env): Promise<Response> {
  const auth = await authenticate(request, env, servicesScope);
  if (auth.status !== 200) {
    return problem(
      auth.status,
      auth.status === 401 ? "unauthorized" : "forbidden",
      auth.body
    );
  }
  const outcome = await runQuery(
    env,
    auth.tenant,
    servicesSql,
    Object.keys(views)
  );
  if (!outcome.ok) {
    return problem(500, "services unavailable", outcome.detail);
  }
  if (outcome.truncated) {
    return problem(
      400,
      "too many services",
      "the number of services exceeds SIGIRO_MAX_RESULT_ROWS"
    );
  }
  const services = outcome.rows.map((row) => ({
    name: row.service_name,
    firstSeen: row.first_seen ?? null,
    lastSeen: row.last_seen ?? null,
    signals: {
      spans: row.span_count,
      logs: row.log_count,
      metrics: row.metric_count,
      profiles: row.profile_count,
    },
    totalCount: row.total_count,
  }));
  return Response.json({ services });
}

async function runQuery(
  env: Env,
  tenant: TenantId,
  sql: string,
  names: string[]
): Promise<QueryOutcome> {
  const sources: Record<string, ViewSource> = {};
  await Promise.all(
    names.map(async (name) => {
      const { table, signal } = views[name];
      const cell = env.INGEST_CELL.get(
        env.INGEST_CELL.idFromName(cellName(tenant, signal))
      );
      sources[name] = {
        keys: await cell.liveKeys(table),
        empty: empty_segment(table),
      };
    })
  );
  return env.QUERY.run({
    sql,
    maxRows: numberVar(env.SIGIRO_MAX_RESULT_ROWS, defaultMaxResultRows),
    views: sources,
  });
}

function referencedViews(sql: string): string[] {
  const lowered = sql.toLowerCase();
  return Object.keys(views).filter((name) =>
    new RegExp(`\\b${name}\\b`, "u").test(lowered)
  );
}

function problem(status: number, title: string, detail: string): Response {
  return Response.json({ status, title, detail }, { status });
}

function mediaType(contentType: string | null): string {
  return contentType?.split(";", 1)[0]?.trim().toLowerCase() ?? "";
}

function parseCellFormat(value: string | null): Format | undefined {
  return value === "protobuf" || value === "json" ? value : undefined;
}

async function decodedBody(
  request: Request,
  limit: number
): Promise<
  { status: 200; body: ArrayBuffer } | { status: 413 | 415; detail: string }
> {
  const encoding =
    request.headers.get("content-encoding")?.trim().toLowerCase() ?? "identity";
  if (encoding !== "identity" && encoding !== "gzip") {
    return { status: 415, detail: `unsupported content encoding ${encoding}` };
  }
  if (request.body === null) {
    return { status: 200, body: new ArrayBuffer(0) };
  }
  const stream =
    encoding === "gzip"
      ? request.body.pipeThrough(new DecompressionStream("gzip"))
      : request.body;
  const chunks: Uint8Array[] = [];
  let size = 0;
  for await (const chunk of stream) {
    size += chunk.byteLength;
    if (size > limit) {
      return { status: 413, detail: "request body exceeds ingest limit" };
    }
    chunks.push(chunk);
  }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return { status: 200, body: body.buffer };
}

function routeSignal(pathname: string): Signal | undefined {
  if (pathname === "/v1/traces") {
    return "traces";
  }
  if (pathname === "/v1/logs") {
    return "logs";
  }
  if (pathname === "/v1/metrics") {
    return "metrics";
  }
  if (pathname === "/v1development/profiles") {
    return "profiles";
  }
  return undefined;
}

function parseSignal(value: string | null): Signal | undefined {
  return value === "traces" ||
    value === "logs" ||
    value === "metrics" ||
    value === "profiles"
    ? value
    : undefined;
}

function parseFormat(mediaType: string): "protobuf" | "json" | undefined {
  if (mediaType === "application/json") {
    return "json";
  }
  if (
    mediaType === "application/x-protobuf" ||
    mediaType === "application/protobuf" ||
    mediaType === "application/octet-stream"
  ) {
    return "protobuf";
  }
  return undefined;
}

async function authenticate(
  request: Request,
  env: Env,
  scope: string
): Promise<
  { status: 200; tenant: TenantId } | { status: 401 | 403; body: string }
> {
  const header = request.headers.get("authorization");
  if (header === null || !header.startsWith("Bearer ")) {
    return { status: 401, body: "missing bearer token" };
  }
  const token = header.slice("Bearer ".length);
  const parts = token.split(".");
  if (parts.length !== 3) {
    return { status: 401, body: "invalid bearer token" };
  }
  const protectedHeader = parseJson<{
    alg?: string;
    kid?: string;
    typ?: string;
  }>(base64urlDecode(parts[0]));
  if (
    protectedHeader === undefined ||
    protectedHeader.alg !== "EdDSA" ||
    protectedHeader.kid === undefined
  ) {
    return { status: 401, body: "invalid bearer token" };
  }
  const jwk = (await fetchJwks(env)).find(
    (key) => key.kid === protectedHeader.kid
  );
  if (jwk === undefined) {
    return { status: 401, body: "invalid bearer token" };
  }
  const valid = await verifyEdDsa(
    jwk,
    base64urlDecode(parts[2]),
    utf8Bytes(`${parts[0]}.${parts[1]}`)
  );
  if (!valid) {
    return { status: 401, body: "invalid bearer token" };
  }
  const claims = parseJson<Claims>(base64urlDecode(parts[1]));
  if (claims === undefined || !validClaims(claims, env)) {
    return { status: 401, body: "invalid bearer token" };
  }
  const tenant = parseTenantId(claims.tid);
  if (tenant === undefined) {
    return { status: 401, body: "invalid bearer token" };
  }
  if (!claims.scope.split(/\s+/u).includes(scope)) {
    return { status: 403, body: `missing ${scope} scope` };
  }
  return { status: 200, tenant };
}

async function verifyEdDsa(
  jwk: Jwk,
  signature: Uint8Array<ArrayBuffer>,
  payload: Uint8Array<ArrayBuffer>
): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey(
      "jwk",
      jwk,
      { name: "Ed25519" },
      false,
      ["verify"]
    );
    return await crypto.subtle.verify(
      { name: "Ed25519" },
      key,
      signature,
      payload
    );
  } catch {
    return verifySignature(
      null,
      payload,
      createPublicKey({ key: jwk as Record<string, unknown>, format: "jwk" }),
      signature
    );
  }
}

async function fetchJwks(env: Env): Promise<Jwk[]> {
  const now = Date.now();
  if (
    jwksCache !== undefined &&
    jwksCache.url === env.SIGIRO_AUTH_JWKS_URL &&
    jwksCache.expiresAt > now
  ) {
    return jwksCache.keys;
  }
  const response = await fetch(env.SIGIRO_AUTH_JWKS_URL);
  if (!response.ok) {
    throw new Error("jwks unavailable");
  }
  const jwks = await response.json<Jwks>();
  jwksCache = {
    url: env.SIGIRO_AUTH_JWKS_URL,
    keys: jwks.keys,
    expiresAt: now + 300_000,
  };
  return jwks.keys;
}

function validClaims(claims: Claims, env: Env): boolean {
  const now = Math.floor(Date.now() / 1000);
  const audience = Array.isArray(claims.aud)
    ? claims.aud.includes(env.SIGIRO_AUTH_AUDIENCE)
    : claims.aud === env.SIGIRO_AUTH_AUDIENCE;
  return (
    claims.iss === env.SIGIRO_AUTH_ISSUER &&
    audience &&
    claims.exp > now &&
    (claims.nbf === undefined || claims.nbf <= now) &&
    claims.sub.trim() !== ""
  );
}

function parseTenantId(value: string | null): TenantId | undefined {
  if (
    value === null ||
    !/^[A-Za-z0-9_]{1,56}$/u.test(value) ||
    value === "extensions" ||
    value === "tenants"
  ) {
    return undefined;
  }
  return value as TenantId;
}

function cellName(tenant: TenantId, signal: Signal): string {
  return `${tenant}:${signal}`;
}

function registerPending(manifest: string, key: string, table: Table): string {
  const parsed = JSON.parse(manifest) as ManifestShape;
  if (parsed.live[key]?.table === table || parsed.pending[key] === table) {
    return manifest;
  }
  return manifest_register(manifest, key, table);
}

function commitLive(
  manifest: string,
  key: string,
  metadata: SegmentMetadata
): string {
  const parsed = JSON.parse(manifest) as ManifestShape;
  if (parsed.live[key]?.table === metadata.table) {
    return manifest;
  }
  return manifest_commit(manifest, JSON.stringify({ key, ...metadata }));
}

function readManifest(sql: SqlStorage): string {
  const row = sql
    .exec<{ json: string }>("SELECT json FROM manifest WHERE id = 1")
    .toArray()[0];
  return row?.json ?? manifest_empty();
}

function writeManifest(sql: SqlStorage, manifest: string): void {
  sql.exec(
    "INSERT INTO manifest (id, json) VALUES (1, ?) ON CONFLICT (id) DO UPDATE SET json = excluded.json",
    manifest
  );
}

function sumWalBytes(sql: SqlStorage): number {
  const row = sql
    .exec<{ total: number }>("SELECT COALESCE(SUM(bytes), 0) AS total FROM wal")
    .one();
  return row.total;
}

function objectKey(
  tenant: TenantId,
  id: number,
  index: number,
  table: Table
): string {
  return `tenants/${tenant}/${table}/${id}-${index}.parquet`;
}

function numberVar(value: string | undefined, fallback: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function utf8Bytes(value: string): Uint8Array<ArrayBuffer> {
  return new Uint8Array(new TextEncoder().encode(value));
}

function base64urlDecode(value: string): Uint8Array<ArrayBuffer> {
  const padded = value
    .replace(/-/gu, "+")
    .replace(/_/gu, "/")
    .padEnd(Math.ceil(value.length / 4) * 4, "=");
  const binary = atob(padded);
  const bytes: Uint8Array<ArrayBuffer> = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function parseJson<T>(bytes: Uint8Array): T | undefined {
  try {
    return JSON.parse(new TextDecoder().decode(bytes)) as T;
  } catch {
    return undefined;
  }
}
