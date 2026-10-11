import { createServer, type Server } from "node:http";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import assert from "node:assert/strict";
import { createPrivateKey, generateKeyPairSync, sign } from "node:crypto";

const root = new URL("..", import.meta.url).pathname;
const cloud = join(root, "cloud");
const esbuild = join(root, "auth/node_modules/esbuild/bin/esbuild");
const port = 9876;
const issuer = "http://127.0.0.1:9999";
const audience = `http://127.0.0.1:${port}`;
const jwksUrl = `${issuer}/jwks`;

const { privateKey, publicKey } = generateKeyPairSync("ed25519");
const jwk = publicKey.export({ format: "jwk" });
const jwks = { keys: [{ ...jwk, kid: "test-key", alg: "EdDSA", use: "sig" }] };

let jwksServer: Server | undefined;
let celld: ChildProcess | undefined;

try {
  jwksServer = await serveJwks();
  writeFileSync(join(cloud, ".dev.vars"), `SIGIRO_AUTH_ISSUER=${issuer}\nSIGIRO_AUTH_AUDIENCE=${audience}\nSIGIRO_AUTH_JWKS_URL=${jwksUrl}\nFLUSH_SECONDS=1\nFLUSH_BYTES=10485760\nSIGIRO_MAX_INGEST_BODY_BYTES=1024\n`);
  celld = await startCelld(true);
  await assertPost("/v1/traces", traceBody("POST /checkout", "checkout"), token("acme", "telemetry:ingest"), 202, "application/x-protobuf");
  const first = await waitForParquet("tenants/acme/spans/");
  assertDuckdb(first, "select span_name, service_name from read_parquet(?)", "span_name,service_name\nPOST /checkout,checkout\n");
  await assertPost("/v1/metrics", metricsBody(), token("acme", "telemetry:ingest"), 202, "application/json");
  await waitForParquet("tenants/acme/metrics_gauge/");
  await waitForParquet("tenants/acme/metrics_sum/");
  await assertPost("/v1/traces", traceBody("acme-only", "checkout"), token("acme", "telemetry:ingest"), 202, "application/x-protobuf");
  await waitForParquet("tenants/acme/spans/", "acme-only");
  assert.equal(listObjects("tenants/globex/spans/").length, 0);
  const beforeBlocked = listObjects("tenants/acme/spans/").length;
  await assertPost("/v1/traces", traceBody("blocked", "checkout"), token("acme", "query:read"), 403, "application/x-protobuf");
  assert.equal(listObjects("tenants/acme/spans/").length, beforeBlocked);
  await assertPost("/v1/traces", traceBody("missing", "checkout"), "", 401, "application/x-protobuf");
  await assertPost("/v1/traces", traceBody("wrong-content", "checkout"), token("acme", "telemetry:ingest"), 415, "text/plain");
  const beforeOversize = listObjects("tenants/acme/spans/").length;
  await assertPost("/v1/traces", new Uint8Array(1_025), token("acme", "telemetry:ingest"), 413, "application/x-protobuf");
  assert.equal(listObjects("tenants/acme/spans/").length, beforeOversize);
  await assertPost("/v1/traces", traceBody("restart-survivor", "checkout"), token("acme", "telemetry:ingest"), 202, "application/x-protobuf");
  celld.kill();
  await waitExit(celld);
  celld = await startCelld(false);
  const restarted = await waitForParquet("tenants/acme/spans/", "restart-survivor");
  assertDuckdb(restarted, "select count(*) as rows from read_parquet(?) where span_name = 'restart-survivor'", "rows\n1\n");
  console.log("cloud ingest e2e passed");
} finally {
  celld?.kill();
  jwksServer?.close();
  rmSync(join(cloud, ".dev.vars"), { force: true });
}

async function serveJwks(): Promise<Server> {
  const server = createServer((request, response) => {
    if (request.url === "/jwks") {
      response.setHeader("content-type", "application/json");
      response.end(JSON.stringify(jwks));
      return;
    }
    response.statusCode = 404;
    response.end("not found");
  });
  await new Promise<void>((resolve) => server.listen(9999, "127.0.0.1", resolve));
  return server;
}

async function startCelld(clean: boolean): Promise<ChildProcess> {
  const args = ["dev", cloud, "--port", String(port), "--no-watch"];
  if (clean) {
    args.push("--clean");
  }
  const child = spawn("celld", args, { env: { ...process.env, CELLD_ESBUILD: esbuild }, stdio: ["ignore", "pipe", "pipe"] });
  let output = "";
  child.stdout?.on("data", (data) => {
    output += data;
    writeFileSync("/tmp/sigiro-cloud-e2e-celld.log", output);
  });
  child.stderr?.on("data", (data) => {
    output += data;
    writeFileSync("/tmp/sigiro-cloud-e2e-celld.log", output);
  });
  for (let attempt = 0; attempt < 60; attempt += 1) {
    if (output.includes(`http://127.0.0.1:${port}`)) {
      return child;
    }
    if (child.exitCode !== null) {
      throw new Error(output);
    }
    await delay(500);
  }
  child.kill();
  throw new Error(output);
}

async function assertPost(path: string, body: Uint8Array, jwt: string, status: number, contentType: string): Promise<void> {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, { method: "POST", headers: { authorization: `Bearer ${jwt}`, "content-type": contentType }, body });
  assert.equal(response.status, status, await response.text());
}

function token(tenant: string, scope: string): string {
  const header = { alg: "EdDSA", kid: "test-key", typ: "JWT" };
  const now = Math.floor(Date.now() / 1000);
  const payload = { iss: issuer, aud: audience, exp: now + 300, nbf: now - 1, sub: "user-1", tid: tenant, scope };
  const signingInput = `${b64(JSON.stringify(header))}.${b64(JSON.stringify(payload))}`;
  const signature = sign(null, Buffer.from(signingInput), createPrivateKey(privateKey.export({ format: "pem", type: "pkcs8" })));
  return `${signingInput}.${b64(signature)}`;
}

async function waitForParquet(prefix: string, contains?: string): Promise<string> {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    for (const key of listObjects(prefix)) {
      const file = readObject(key);
      if (contains === undefined || duckdbContains(file, contains)) {
        return file;
      }
    }
    await delay(500);
  }
  throw new Error(`missing object ${prefix}`);
}

function listObjects(prefix: string): string[] {
  const db = join(cloud, ".celld/dev/objects.sqlite3");
  const storedPrefix = `r2/sigiro-segments-dev/${prefix}`;
  const sql = `select key from objects where key like '${storedPrefix.replaceAll("'", "''")}%' order by key`;
  const result = spawnSync("sqlite3", ["-cmd", ".timeout 10000", db, sql], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim() === "" ? [] : result.stdout.trim().split("\n");
}

function readObject(key: string): string {
  const db = join(cloud, ".celld/dev/objects.sqlite3");
  const dir = mkdtempSync(join(tmpdir(), "sigiro-parquet-"));
  const file = join(dir, "segment.parquet");
  const sql = `select writefile('${file.replaceAll("'", "''")}', body) from objects where key = '${key.replaceAll("'", "''")}';\n`;
  const result = spawnSync("sqlite3", ["-bail", "-cmd", ".timeout 10000", db], { input: sql, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return file;
}

function assertDuckdb(file: string, sql: string, expected: string): void {
  const result = spawnSync("duckdb", ["-csv", "-c", sql.replace("?", `'${file.replaceAll("'", "''")}'`)], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, expected);
  rmSync(new URL(".", `file://${file}`).pathname, { recursive: true, force: true });
}

function duckdbContains(file: string, value: string): boolean {
  const result = spawnSync("duckdb", ["-csv", "-c", `select count(*) from '${file.replaceAll("'", "''")}' where span_name = '${value.replaceAll("'", "''")}'`], { encoding: "utf8" });
  return result.status === 0 && result.stdout.trim().endsWith("1");
}

function traceBody(name: string, service: string): Uint8Array {
  const span = message([bytesField(1, hex("1032547698badcfe1032547698badcfe")), bytesField(2, hex("0123456789abcdef")), stringField(5, name), varintField(6, 2n), fixed64Field(7, 1_700_000_000_000_001_000n), fixed64Field(8, 1_700_000_000_000_009_000n)]);
  const scopeSpans = message([messageField(2, span)]);
  const resource = message([messageField(1, keyValue("service.name", service))]);
  const resourceSpans = message([messageField(1, resource), messageField(2, scopeSpans)]);
  return message([messageField(1, resourceSpans)]);
}

function metricsBody(): Uint8Array {
  const time = "1700000000000001000";
  const resource = { attributes: [{ key: "service.name", value: { stringValue: "checkout" } }] };
  return new TextEncoder().encode(JSON.stringify({ resourceMetrics: [{ resource, scopeMetrics: [{ metrics: [{ name: "cart.items", gauge: { dataPoints: [{ timeUnixNano: time, startTimeUnixNano: time, asDouble: 2 }] } }, { name: "checkout.total", sum: { dataPoints: [{ timeUnixNano: time, startTimeUnixNano: time, asDouble: 4 }], aggregationTemporality: 1, isMonotonic: true } }] }] }] }));
}

function keyValue(key: string, value: string): Uint8Array {
  return message([stringField(1, key), messageField(2, message([stringField(1, value)]))]);
}

function message(fields: Uint8Array[]): Uint8Array {
  return concat(fields);
}

function messageField(field: number, value: Uint8Array): Uint8Array {
  return concat([tag(field, 2), varint(BigInt(value.length)), value]);
}

function stringField(field: number, value: string): Uint8Array {
  return messageField(field, new TextEncoder().encode(value));
}

function bytesField(field: number, value: Uint8Array): Uint8Array {
  return messageField(field, value);
}

function varintField(field: number, value: bigint): Uint8Array {
  return concat([tag(field, 0), varint(value)]);
}

function fixed64Field(field: number, value: bigint): Uint8Array {
  const out = new Uint8Array(9);
  out[0] = field * 8 + 1;
  new DataView(out.buffer).setBigUint64(1, value, true);
  return out;
}

function tag(field: number, wire: number): Uint8Array {
  return varint(BigInt(field * 8 + wire));
}

function varint(value: bigint): Uint8Array {
  const bytes: number[] = [];
  let current = value;
  while (current >= 0x80n) {
    bytes.push(Number((current & 0x7fn) | 0x80n));
    current >>= 7n;
  }
  bytes.push(Number(current));
  return new Uint8Array(bytes);
}

function concat(parts: Uint8Array[]): Uint8Array {
  const size = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(size);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function hex(value: string): Uint8Array {
  return Uint8Array.from(value.match(/../gu)?.map((part) => Number.parseInt(part, 16)) ?? []);
}

function b64(value: string | Buffer): string {
  return Buffer.from(value).toString("base64url");
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitExit(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null) {
    return;
  }
  await new Promise<void>((resolve) => child.once("exit", () => resolve()));
}
