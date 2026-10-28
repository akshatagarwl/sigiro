import { createServer, type Server } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import assert from "node:assert/strict";
import { createPrivateKey, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { gzipSync } from "node:zlib";

const root = new URL("../..", import.meta.url).pathname;
const cloud = join(root, "cloud");
const esbuild = join(root, "auth/node_modules/esbuild/bin/esbuild");
const segmentsBucket = "sigiro-segments-dev";
const s3Port = 19100 + Math.floor(Math.random() * 400);
const jwksPort = s3Port + 500;
const nodePort = s3Port + 1000;
export const origin = `http://127.0.0.1:${nodePort}`;
const issuer = `http://127.0.0.1:${jwksPort}`;
const credentials = { AWS_ACCESS_KEY_ID: "sigiro-e2e", AWS_SECRET_ACCESS_KEY: randomBytes(12).toString("hex"), AWS_REGION: "us-east-1" };
const fleetArgs = ["--bucket", "s3://sigiro-e2e-fleet", "--endpoint", `http://127.0.0.1:${s3Port}`, "--region", "us-east-1"];
const container = `sigiro-e2e-rustfs-${s3Port}`;
const { privateKey, publicKey } = generateKeyPairSync("ed25519");

export type FleetVars = { FLUSH_SECONDS?: string; SIGIRO_MAX_INGEST_BODY_BYTES?: string; SIGIRO_MAX_RESULT_ROWS?: string; QUERY_MAX_SCAN_BYTES?: string; QUERY_MEMORY_LIMIT?: string };

export class Fleet {
  private node: ChildProcess | undefined;
  private jwks: Server | undefined;
  private log = "";

  async start(vars: FleetVars): Promise<void> {
    run("docker", ["rm", "-f", container]);
    run("docker", ["run", "-d", "--name", container, "-p", `${s3Port}:9000`, "-e", `RUSTFS_ACCESS_KEY=${credentials.AWS_ACCESS_KEY_ID}`, "-e", `RUSTFS_SECRET_KEY=${credentials.AWS_SECRET_ACCESS_KEY}`, "rustfs/rustfs:1.0.0"], true);
    await waitFor(async () => (await fetch(`http://127.0.0.1:${s3Port}/`).catch(() => undefined)) !== undefined, "rustfs");
    run("docker", ["run", "--rm", "--network", "host", "-e", `RC_HOST_local=http://${credentials.AWS_ACCESS_KEY_ID}:${credentials.AWS_SECRET_ACCESS_KEY}@127.0.0.1:${s3Port}`, "rustfs/rc:latest", "mb", "local/sigiro-e2e-fleet"], true);
    this.jwks = await serveJwks();
    this.deploy(vars);
    await this.boot();
  }

  deployApiRevision(revision: string, vars: FleetVars): void {
    const project = join(cloud, `.e2e-${revision}`);
    rmSync(project, { recursive: true, force: true });
    mkdirSync(join(project, "src"), { recursive: true });
    writeFileSync(join(project, "src/worker.ts"), run("git", ["-C", root, "show", `${revision}:cloud/src/worker.ts`], true));
    for (const shared of ["vendor", "query", "types", "node_modules"]) {
      symlinkSync(join(cloud, shared), join(project, shared));
    }
    try {
      this.deployProject(project, "src/worker.ts", vars);
    } finally {
      rmSync(project, { recursive: true, force: true });
    }
  }

  deploy(vars: FleetVars): void {
    this.deployProject(join(cloud, "query"), "src/query.ts", vars);
    this.deployProject(cloud, "src/worker.ts", vars);
  }

  private deployProject(dir: string, main: string, vars: FleetVars): void {
    const base = { SIGIRO_AUTH_ISSUER: issuer, SIGIRO_AUTH_AUDIENCE: origin, SIGIRO_AUTH_JWKS_URL: `${issuer}/jwks`, FLUSH_BYTES: "10485760" };
    const source = dir.includes(".e2e-") ? cloud : dir;
    const config = JSON.parse(readFileSync(join(source, "wrangler.jsonc"), "utf8"));
    config.main = main;
    delete config.workers_dev;
    config.vars = { ...config.vars, ...base, ...vars };
    config.r2_buckets = config.r2_buckets.map((bucket: { binding: string }) => ({ ...bucket, bucket_name: segmentsBucket }));
    const path = join(dir, "wrangler.e2e.json");
    writeFileSync(path, JSON.stringify(config));
    try {
      run("celld", ["deploy", "--config", path, ...fleetArgs], true);
    } finally {
      rmSync(path, { force: true });
    }
  }

  async restart(signal: "SIGTERM" | "SIGKILL"): Promise<void> {
    await this.stopNode(signal);
    await this.boot();
  }

  async stop(): Promise<void> {
    await this.stopNode("SIGKILL");
    this.jwks?.close();
    run("docker", ["rm", "-f", container]);
  }

  keys(prefix: string): string[] {
    const out = run("celld", ["r2", "list", segmentsBucket, "--prefix", prefix, ...fleetArgs], true);
    return out.split("\n").filter((line) => line.startsWith(prefix));
  }

  download(keys: string[]): string {
    const dir = mkdtempSync(join(tmpdir(), "sigiro-e2e-parquet-"));
    for (const [index, key] of keys.entries()) {
      const result = spawnSync("celld", ["r2", "get", segmentsBucket, key, ...fleetArgs], { env: env(), maxBuffer: 1 << 28 });
      assert.equal(result.status, 0, String(result.stderr));
      writeFileSync(join(dir, `${index}.parquet`), result.stdout);
    }
    return dir;
  }

  count(prefix: string, column: string, value: string): number {
    const keys = this.keys(prefix);
    if (keys.length === 0) {
      return 0;
    }
    const dir = this.download(keys);
    try {
      return Number(duckdb(`select count(*) from read_parquet('${dir}/*.parquet') where ${column} = '${value.replaceAll("'", "''")}'`).split("\n")[1]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }

  async waitForRow(prefix: string, column: string, value: string): Promise<void> {
    await waitFor(async () => this.count(prefix, column, value) > 0, `${prefix} ${column}=${value}`, 120);
  }

  private async boot(): Promise<void> {
    const watch = mkdtempSync(join(tmpdir(), "sigiro-e2e-celld-"));
    this.log = "";
    const child = spawn("celld", [...fleetArgs, "--listen", `127.0.0.1:${nodePort}`], { env: { ...env(), CELLD_WATCH: watch }, stdio: ["ignore", "pipe", "pipe"] });
    child.stdout?.on("data", (data) => this.capture(data));
    child.stderr?.on("data", (data) => this.capture(data));
    this.node = child;
    await waitFor(async () => {
      if (child.exitCode !== null) {
        throw new Error(this.log);
      }
      const response = await fetch(`${origin}/v1/services`).catch(() => undefined);
      return response !== undefined && response.status === 401;
    }, "celld node");
  }

  private capture(data: Buffer): void {
    this.log += data;
    writeFileSync("/tmp/sigiro-cloud-e2e-celld.log", this.log);
  }

  private async stopNode(signal: "SIGTERM" | "SIGKILL"): Promise<void> {
    const node = this.node;
    if (node === undefined || node.exitCode !== null) {
      return;
    }
    const exited = new Promise<void>((resolve) => node.once("exit", () => resolve()));
    node.kill(signal);
    await exited;
  }
}

export function token(tenant: string, scope: string): string {
  const header = { alg: "EdDSA", kid: "test-key", typ: "JWT" };
  const now = Math.floor(Date.now() / 1000);
  const payload = { iss: issuer, aud: origin, exp: now + 600, nbf: now - 1, sub: "user-1", tid: tenant, scope };
  const signingInput = `${b64(JSON.stringify(header))}.${b64(JSON.stringify(payload))}`;
  const signature = sign(null, Buffer.from(signingInput), createPrivateKey(privateKey.export({ format: "pem", type: "pkcs8" })));
  return `${signingInput}.${b64(signature)}`;
}

export async function post(path: string, body: Uint8Array | string | FormData, jwt: string, contentType?: string, extra: Record<string, string> = {}): Promise<Response> {
  const headers: Record<string, string> = { authorization: `Bearer ${jwt}`, ...extra };
  if (contentType !== undefined) {
    headers["content-type"] = contentType;
  }
  return fetch(`${origin}${path}`, { method: "POST", headers, body });
}

export async function expectStatus(response: Response, status: number): Promise<string> {
  const text = await response.text();
  assert.equal(response.status, status, text);
  return text;
}

export async function postUntilAccepted(path: string, body: Uint8Array | string, jwt: string, contentType: string): Promise<number> {
  const seen: number[] = [];
  let last = "";
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const response = await post(path, body, jwt, contentType);
    const text = await response.text();
    last = text;
    seen.push(response.status);
    assert.ok(response.status === 202 || response.status === 503, `status ${response.status} is neither accepted nor retryable: ${text}`);
    if (response.status === 503) {
      assert.ok(response.headers.has("retry-after"), "503 carries retry-after");
    }
    if (response.status === 202) {
      return seen.length;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(`export never accepted after ${seen.length} attempts (${[...new Set(seen)].join(",")}): ${last}`);
}

export function duckdb(sql: string): string {
  const result = spawnSync("duckdb", ["-csv", "-c", sql], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

export function traceBody(spans: { name: string; service: string }[]): Uint8Array {
  const resourceSpans = spans.map(({ name, service }, index) => {
    const span = message([bytesField(1, hex("1032547698badcfe1032547698badcfe")), bytesField(2, hex(`0123456789abcd${String(index).padStart(2, "0")}`)), stringField(5, name), varintField(6, 2n), fixed64Field(7, 1_700_000_000_000_001_000n + BigInt(index) * 1000n), fixed64Field(8, 1_700_000_000_000_009_000n + BigInt(index) * 1000n)]);
    const resource = message([messageField(1, keyValue("service.name", service))]);
    return messageField(1, message([messageField(1, resource), messageField(2, message([messageField(2, span)]))]));
  });
  return message(resourceSpans);
}

export function logsBody(service: string, body: string): string {
  const resource = { attributes: [{ key: "service.name", value: { stringValue: service } }] };
  return JSON.stringify({ resourceLogs: [{ resource, scopeLogs: [{ logRecords: [{ timeUnixNano: "1700000000000002000", severityNumber: 9, body: { stringValue: body } }] }] }] });
}

export function metricsBody(service: string): string {
  const time = "1700000000000001000";
  const resource = { attributes: [{ key: "service.name", value: { stringValue: service } }] };
  return JSON.stringify({ resourceMetrics: [{ resource, scopeMetrics: [{ metrics: [{ name: "cart.items", gauge: { dataPoints: [{ timeUnixNano: time, startTimeUnixNano: time, asDouble: 2 }] } }, { name: "checkout.total", sum: { dataPoints: [{ timeUnixNano: time, startTimeUnixNano: time, asDouble: 4 }], aggregationTemporality: 1, isMonotonic: true } }] }] }] });
}

export function otlpProfileBody(service: string): Uint8Array {
  const strings = ["", "cpu", "nanoseconds", "main", "work"];
  const fn = (name: number) => message([varintField(1, BigInt(name))]);
  const location = (fnIndex: number) => message([messageField(3, message([varintField(1, BigInt(fnIndex))]))]);
  const stack = (indices: number[]) => message(indices.map((index) => varintField(1, BigInt(index))));
  const dictionary = message([
    ...[message([]), location(1), location(2)].map((entry) => messageField(2, entry)),
    ...[fn(0), fn(3), fn(4)].map((entry) => messageField(3, entry)),
    ...strings.map((value) => stringField(5, value)),
    ...[stack([]), stack([2, 1]), stack([1])].map((entry) => messageField(7, entry)),
  ]);
  const sample = (stackIndex: number, value: number) => message([varintField(1, BigInt(stackIndex)), varintField(2, BigInt(value))]);
  const profile = message([messageField(1, message([varintField(1, 1n), varintField(2, 2n)])), messageField(2, sample(1, 10)), messageField(2, sample(2, 5)), fixed64Field(3, 1_700_000_000_000_000_000n)]);
  const resource = message([messageField(1, keyValue("service.name", service))]);
  const resourceProfiles = message([messageField(1, resource), messageField(2, message([messageField(2, profile)]))]);
  return message([messageField(1, resourceProfiles), messageField(2, dictionary)]);
}

export function pprofBody(): Uint8Array {
  const strings = ["", "cpu", "nanoseconds", "main", "work"];
  const location = (id: number, fnId: number) => message([varintField(1, BigInt(id)), messageField(4, message([varintField(1, BigInt(fnId))]))]);
  const fn = (id: number, name: number) => message([varintField(1, BigInt(id)), varintField(2, BigInt(name))]);
  const sample = (ids: number[], value: number) => message([...ids.map((id) => varintField(1, BigInt(id))), varintField(2, BigInt(value))]);
  return message([
    messageField(1, message([varintField(1, 1n), varintField(2, 2n)])),
    messageField(2, sample([2, 1], 10)),
    messageField(2, sample([1], 5)),
    messageField(4, location(1, 1)),
    messageField(4, location(2, 2)),
    messageField(5, fn(1, 3)),
    messageField(5, fn(2, 4)),
    ...strings.map((value) => stringField(6, value)),
    varintField(9, 1_700_000_000_000_000_000n),
  ]);
}

export function pushRequestBody(service: string): Uint8Array {
  const label = message([stringField(1, "service_name"), stringField(2, service)]);
  return message([messageField(1, message([messageField(1, label), messageField(2, message([bytesField(1, pprofBody())]))]))]);
}

export function gzip(body: Uint8Array | string): Uint8Array {
  return new Uint8Array(gzipSync(body));
}

export async function waitFor(check: () => Promise<boolean>, what: string, seconds = 60): Promise<void> {
  for (let attempt = 0; attempt < seconds * 2; attempt += 1) {
    if (await check()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`timed out waiting for ${what}`);
}

function env(): NodeJS.ProcessEnv {
  return { ...process.env, ...credentials, CELLD_ESBUILD: esbuild };
}

function run(command: string, args: string[], check = false): string {
  const result = spawnSync(command, args, { env: env(), encoding: "utf8", maxBuffer: 1 << 26 });
  if (check) {
    assert.equal(result.status, 0, `${command} ${args.join(" ")}\n${result.stderr}`);
  }
  return result.stdout ?? "";
}

async function serveJwks(): Promise<Server> {
  const jwks = { keys: [{ ...publicKey.export({ format: "jwk" }), kid: "test-key", alg: "EdDSA", use: "sig" }] };
  const server = createServer((request, response) => {
    response.statusCode = request.url === "/jwks" ? 200 : 404;
    response.setHeader("content-type", "application/json");
    response.end(request.url === "/jwks" ? JSON.stringify(jwks) : "{}");
  });
  await new Promise<void>((resolve) => server.listen(jwksPort, "127.0.0.1", resolve));
  return server;
}

function keyValue(key: string, value: string): Uint8Array {
  return message([stringField(1, key), messageField(2, message([stringField(1, value)]))]);
}

function message(fields: Uint8Array[]): Uint8Array {
  const size = fields.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(size);
  let offset = 0;
  for (const part of fields) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function messageField(field: number, value: Uint8Array): Uint8Array {
  return message([varint(BigInt(field * 8 + 2)), varint(BigInt(value.length)), value]);
}

function stringField(field: number, value: string): Uint8Array {
  return messageField(field, new TextEncoder().encode(value));
}

function bytesField(field: number, value: Uint8Array): Uint8Array {
  return messageField(field, value);
}

function varintField(field: number, value: bigint): Uint8Array {
  return message([varint(BigInt(field * 8)), varint(value)]);
}

function fixed64Field(field: number, value: bigint): Uint8Array {
  const out = new Uint8Array(9);
  out[0] = field * 8 + 1;
  new DataView(out.buffer).setBigUint64(1, value, true);
  return out;
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

function hex(value: string): Uint8Array {
  return Uint8Array.from(value.match(/../gu)?.map((part) => Number.parseInt(part, 16)) ?? []);
}

function b64(value: string | Buffer): string {
  return Buffer.from(value).toString("base64url");
}
