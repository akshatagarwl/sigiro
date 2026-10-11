import { DurableObject } from "cloudflare:workers";
import { createPublicKey, verify as verifySignature } from "node:crypto";
import initWasm, { decode_otlp, manifest_commit, manifest_empty, manifest_register } from "../vendor/segments-wasm/sigiro_segments_wasm.js";
import wasmModule from "../vendor/segments-wasm/sigiro_segments_wasm_bg.wasm";

await initWasm(wasmModule);

type Signal = "traces" | "logs" | "metrics";
type Format = "protobuf" | "json";
type Table = "spans" | "logs" | "metrics_gauge" | "metrics_sum" | "metrics_histogram" | "metrics_exp_histogram" | "profiles";

type Env = {
  INGEST_CELL: DurableObjectNamespace<IngestCell>;
  SEGMENTS: R2Bucket;
  SIGIRO_AUTH_ISSUER: string;
  SIGIRO_AUTH_AUDIENCE: string;
  SIGIRO_AUTH_JWKS_URL: string;
  SIGIRO_MAX_INGEST_BODY_BYTES?: string;
  FLUSH_SECONDS?: string;
  FLUSH_BYTES?: string;
};

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
type WalRow = { id: number; tenant: string; signal: Signal; format: Format; body: ArrayBuffer; bytes: number };
type SegmentMetadata = { table: Table; min_time_us: number; max_time_us: number; rows: number; bytes: number };
type ManifestShape = { pending: Record<string, Table>; live: Record<string, { table: Table }> };

const ingestScope = "telemetry:ingest";
const defaultBodyLimit = 8_388_608;
const defaultFlushSeconds = 60;
const defaultFlushBytes = 4_194_304;

let jwksCache: { url: string; keys: Jwk[]; expiresAt: number } | undefined;

export class IngestCell extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS wal (id INTEGER PRIMARY KEY AUTOINCREMENT, tenant TEXT NOT NULL, signal TEXT NOT NULL, format TEXT NOT NULL, body BLOB NOT NULL, bytes INTEGER NOT NULL)");
    this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS manifest (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL)");
    this.ctx.storage.sql.exec("INSERT OR IGNORE INTO manifest (id, json) VALUES (1, ?)", manifest_empty());
  }

  async fetch(request: Request): Promise<Response> {
    if (request.method !== "POST") {
      return new Response("method not allowed", { status: 405 });
    }
    const params = new URL(request.url).searchParams;
    const signal = parseSignal(params.get("signal"));
    const tenant = params.get("tenant");
    const format = parseFormat(request.headers.get("content-type"));
    if (signal === undefined || format === undefined || tenant === null || tenant.trim() === "") {
      return new Response("bad ingest cell request", { status: 400 });
    }
    const body = await request.arrayBuffer();
    try {
      decode_otlp(signal, format, new Uint8Array(body));
    } catch (error) {
      return new Response(String(error), { status: 400 });
    }
    this.ctx.storage.sql.exec("INSERT INTO wal (tenant, signal, format, body, bytes) VALUES (?, ?, ?, ?, ?)", tenant, signal, format, body, body.byteLength);
    await this.ctx.storage.sync?.();
    await this.scheduleIfNeeded();
    return new Response(null, { status: 202 });
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
      await this.ctx.storage.setAlarm(Date.now() + numberVar(this.env.FLUSH_SECONDS, defaultFlushSeconds) * 1000);
    }
  }

  private async flush(): Promise<void> {
    const rows = this.ctx.storage.sql.exec<WalRow>("SELECT id, tenant, signal, format, body, bytes FROM wal ORDER BY id").toArray();
    if (rows.length === 0) {
      return;
    }
    let manifest = readManifest(this.ctx.storage.sql);
    for (const row of rows) {
      const outputs = decode_otlp(row.signal, row.format, new Uint8Array(row.body));
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
    const route = routeSignal(url.pathname);
    if (request.method !== "POST" || route === undefined) {
      return new Response("not found", { status: 404 });
    }
    const limit = numberVar(env.SIGIRO_MAX_INGEST_BODY_BYTES, defaultBodyLimit);
    const length = request.headers.get("content-length");
    if (length !== null && Number(length) > limit) {
      return new Response("request body exceeds ingest limit", { status: 413 });
    }
    const format = parseFormat(request.headers.get("content-type"));
    if (format === undefined) {
      return new Response("unsupported content type", { status: 415 });
    }
    const auth = await authenticate(request, env);
    if (auth.status !== 200) {
      return new Response(auth.body, { status: auth.status });
    }
    const body = await boundedBody(request, limit);
    if (body === undefined) {
      return new Response("request body exceeds ingest limit", { status: 413 });
    }
    const id = env.INGEST_CELL.idFromName(`${auth.tenant}:${route}`);
    const stub = env.INGEST_CELL.get(id);
    return stub.fetch(`https://cell.invalid/ingest?signal=${route}&tenant=${encodeURIComponent(auth.tenant)}`, { method: "POST", headers: { "content-type": format }, body });
  },
};

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
  return undefined;
}

function parseSignal(value: string | null): Signal | undefined {
  return value === "traces" || value === "logs" || value === "metrics" ? value : undefined;
}

function parseFormat(contentType: string | null): Format | undefined {
  const value = contentType?.split(";", 1)[0]?.trim().toLowerCase();
  if (value === "application/json" || value === "json") {
    return "json";
  }
  if (value === "application/x-protobuf" || value === "application/protobuf" || value === "application/octet-stream" || value === "protobuf") {
    return "protobuf";
  }
  return undefined;
}

async function authenticate(request: Request, env: Env): Promise<{ status: 200; tenant: string } | { status: 401 | 403; body: string }> {
  const header = request.headers.get("authorization");
  if (header === null || !header.startsWith("Bearer ")) {
    return { status: 401, body: "missing bearer token" };
  }
  const token = header.slice("Bearer ".length);
  const parts = token.split(".");
  if (parts.length !== 3) {
    return { status: 401, body: "invalid bearer token" };
  }
  const protectedHeader = parseJson<{ alg?: string; kid?: string; typ?: string }>(base64urlDecode(parts[0]));
  if (protectedHeader === undefined || protectedHeader.alg !== "EdDSA" || protectedHeader.kid === undefined) {
    return { status: 401, body: "invalid bearer token" };
  }
  const jwk = (await fetchJwks(env)).find((key) => key.kid === protectedHeader.kid);
  if (jwk === undefined) {
    return { status: 401, body: "invalid bearer token" };
  }
  const valid = await verifyEdDsa(jwk, base64urlDecode(parts[2]), utf8Bytes(`${parts[0]}.${parts[1]}`));
  if (!valid) {
    return { status: 401, body: "invalid bearer token" };
  }
  const claims = parseJson<Claims>(base64urlDecode(parts[1]));
  if (claims === undefined || !validClaims(claims, env)) {
    return { status: 401, body: "invalid bearer token" };
  }
  if (!claims.scope.split(/\s+/u).includes(ingestScope)) {
    return { status: 403, body: "missing telemetry ingest scope" };
  }
  return { status: 200, tenant: claims.tid };
}

async function verifyEdDsa(jwk: Jwk, signature: Uint8Array<ArrayBuffer>, payload: Uint8Array<ArrayBuffer>): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey("jwk", jwk, { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, key, signature, payload);
  } catch {
    return verifySignature(null, payload, createPublicKey({ key: jwk as Record<string, unknown>, format: "jwk" }), signature);
  }
}

async function fetchJwks(env: Env): Promise<Jwk[]> {
  const now = Date.now();
  if (jwksCache !== undefined && jwksCache.url === env.SIGIRO_AUTH_JWKS_URL && jwksCache.expiresAt > now) {
    return jwksCache.keys;
  }
  const response = await fetch(env.SIGIRO_AUTH_JWKS_URL);
  if (!response.ok) {
    throw new Error("jwks unavailable");
  }
  const jwks = await response.json<Jwks>();
  jwksCache = { url: env.SIGIRO_AUTH_JWKS_URL, keys: jwks.keys, expiresAt: now + 300_000 };
  return jwks.keys;
}

function validClaims(claims: Claims, env: Env): boolean {
  const now = Math.floor(Date.now() / 1000);
  const audience = Array.isArray(claims.aud) ? claims.aud.includes(env.SIGIRO_AUTH_AUDIENCE) : claims.aud === env.SIGIRO_AUTH_AUDIENCE;
  return claims.iss === env.SIGIRO_AUTH_ISSUER && audience && claims.exp > now && (claims.nbf === undefined || claims.nbf <= now) && claims.sub.trim() !== "" && claims.tid.trim() !== "";
}

async function boundedBody(request: Request, limit: number): Promise<ArrayBuffer | undefined> {
  const body = await request.arrayBuffer();
  return body.byteLength > limit ? undefined : body;
}

function registerPending(manifest: string, key: string, table: Table): string {
  const parsed = JSON.parse(manifest) as ManifestShape;
  if (parsed.live[key]?.table === table || parsed.pending[key] === table) {
    return manifest;
  }
  return manifest_register(manifest, key, table);
}

function commitLive(manifest: string, key: string, metadata: SegmentMetadata): string {
  const parsed = JSON.parse(manifest) as ManifestShape;
  if (parsed.live[key]?.table === metadata.table) {
    return manifest;
  }
  return manifest_commit(manifest, JSON.stringify({ key, ...metadata }));
}

function readManifest(sql: SqlStorage): string {
  const row = sql.exec<{ json: string }>("SELECT json FROM manifest WHERE id = 1").one();
  return row.json;
}

function writeManifest(sql: SqlStorage, manifest: string): void {
  sql.exec("UPDATE manifest SET json = ? WHERE id = 1", manifest);
}

function sumWalBytes(sql: SqlStorage): number {
  const row = sql.exec<{ total: number }>("SELECT COALESCE(SUM(bytes), 0) AS total FROM wal").one();
  return row.total;
}

function objectKey(tenant: string, id: number, index: number, table: Table): string {
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
  const padded = value.replace(/-/gu, "+").replace(/_/gu, "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
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
