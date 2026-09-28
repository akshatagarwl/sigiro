import { createHash, randomUUID } from "node:crypto";
import { lstat, mkdir, open, readFile, rename, rm, writeFile } from "node:fs/promises";
import { setTimeout as sleep } from "node:timers/promises";
import path from "node:path";
import { z } from "zod";
import type { SandboxBackend, SandboxSession } from "eve/sandbox";
import { checkpointKey, decryptCheckpoint, encryptCheckpoint } from "./checkpoint.ts";

const MAX_FILE_BYTES = 2 * 1024 * 1024;
const MAX_ARCHIVE_BYTES = 32 * 1024 * 1024;
const MAX_OUTPUT_BYTES = 1024 * 1024;
const Identifier = z.string().regex(/^[a-zA-Z0-9_-]{1,128}$/);

function resolvePath(input: string): string {
  const resolved = path.posix.resolve("/workspace", input);
  if (resolved !== "/workspace" && !resolved.startsWith("/workspace/")) throw new Error("Sandbox path escapes /workspace");
  return resolved;
}

const filePath = (input: string) => resolvePath(input).split("/").map(encodeURIComponent).join("/");
const shellQuote = (input: string) => `'${input.replaceAll("'", "'\\''")}'`;

async function bounded(response: Response, limit: number): Promise<Buffer> {
  if (!response.body) throw new Error("Sandbox returned no response body");
  const chunks: Uint8Array[] = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.byteLength;
    if (size > limit) {
      await response.body.cancel().catch(() => {});
      throw new Error("Sandbox payload exceeded its size limit");
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

export class CloudflareBridge {
  private readonly root: string;
  private readonly key: string;
  constructor(root: string, key: string) {
    const url = new URL(root);
    if (url.username || url.password || url.search || url.hash || url.pathname !== "/" ||
      (url.protocol !== "https:" && !(url.protocol === "http:" && ["127.0.0.1", "localhost", "::1"].includes(url.hostname)))) {
      throw new Error("Sandbox bridge must use HTTPS or loopback without URL credentials");
    }
    if (key.length < 32) throw new Error("Sandbox bridge key must be at least 32 characters");
    this.root = url.origin;
    this.key = key;
  }

  async request(method: string, route: string, body?: BodyInit, signal?: AbortSignal): Promise<Response> {
    const response = await fetch(`${this.root}${route}`, {
      method, body, redirect: "error", signal: signal ?? AbortSignal.timeout(130_000),
      headers: { Authorization: `Bearer ${this.key}`, ...(body instanceof Blob ? { "Content-Type": "application/octet-stream" } :
        typeof body === "string" ? { "Content-Type": "application/json" } : {}) },
    });
    if (!response.ok && !(response.status === 404 && (method === "GET" || method === "DELETE"))) {
      await response.body?.cancel().catch(() => {});
      const operation = route.replace(/\/v1\/sandbox\/[a-zA-Z0-9_-]+/, "/v1/sandbox/{id}");
      throw new Error(`Sandbox bridge ${method} ${operation} rejected (${response.status})`);
    }
    return response;
  }

  async create(): Promise<string> {
    const response = await this.request("POST", "/v1/sandbox");
    const data: unknown = await response.json();
    return Identifier.parse(z.object({ id: z.string() }).parse(data).id);
  }

  async running(id: string): Promise<boolean> {
    const response = await this.request("GET", `/v1/sandbox/${Identifier.parse(id)}/running`);
    if (response.status === 404) return false;
    const data: unknown = await response.json();
    return z.object({ running: z.boolean() }).parse(data).running;
  }

  async destroy(id: string): Promise<void> {
    await this.request("DELETE", `/v1/sandbox/${Identifier.parse(id)}`);
  }

  async read(id: string, filename: string, signal?: AbortSignal): Promise<Buffer | null> {
    const quoted = shellQuote(resolvePath(filename));
    const inspected = await runCommand(this, id, `if test -L ${quoted}; then exit 45; elif test -f ${quoted}; then wc -c < ${quoted}; else exit 44; fi`, "/workspace", signal);
    if (inspected.exitCode === 44) return null;
    if (inspected.exitCode !== 0) throw new Error("Sandbox file cannot be read safely");
    const expected = Number(inspected.stdout.trim());
    if (!Number.isSafeInteger(expected) || expected < 0 || expected > MAX_FILE_BYTES) throw new Error("Sandbox file exceeds size limit");
    const response = await this.request("GET", `/v1/sandbox/${Identifier.parse(id)}/file${filePath(filename)}`, undefined, signal);
    const bytes = response.status === 404 ? null : await bounded(response, MAX_FILE_BYTES);
    if (bytes?.byteLength !== expected) throw new Error("Sandbox file changed or could not be read");
    return bytes;
  }

  async write(id: string, filename: string, bytes: Uint8Array, signal?: AbortSignal): Promise<void> {
    if (bytes.byteLength > MAX_FILE_BYTES) throw new Error("Sandbox file exceeds size limit");
    const response = await this.request("PUT", `/v1/sandbox/${Identifier.parse(id)}/file${filePath(filename)}`,
      new Blob([new Uint8Array(bytes)]), signal);
    await response.body?.cancel();
  }

  async persist(id: string): Promise<Buffer> {
    return bounded(await this.request("POST", `/v1/sandbox/${Identifier.parse(id)}/persist`), MAX_ARCHIVE_BYTES);
  }

  async hydrate(id: string, archive: Buffer): Promise<void> {
    if (archive.byteLength > MAX_ARCHIVE_BYTES) throw new Error("Sandbox checkpoint exceeds size limit");
    for (let attempt = 0; attempt < 20; attempt++) {
      try {
        const response = await this.request("POST", `/v1/sandbox/${Identifier.parse(id)}/hydrate`, new Blob([new Uint8Array(archive)]));
        await response.body?.cancel();
        return;
      } catch (error) {
        if (!(error instanceof Error && error.message.endsWith("rejected (503)")) || attempt === 19) throw error;
        await sleep(1000);
      }
    }
  }

  async exec(id: string, command: string, cwd: string, signal?: AbortSignal): Promise<Response> {
    if (command.length > 16_384) throw new Error("Sandbox command exceeds size limit");
    // Cloudflare's internet policy leaves DNS as an exception. Put *every*
    // untrusted command in a fresh network namespace with no external interface.
    // If unprivileged namespaces are unavailable, unshare fails instead of
    // silently running the command with DNS access.
    return this.request("POST", `/v1/sandbox/${Identifier.parse(id)}/exec`,
      JSON.stringify({ argv: ["unshare", "--user", "--map-root-user", "--net", "--", "sh", "-lc", command], cwd: resolvePath(cwd), timeout_ms: 120_000 }), signal);
  }
}

async function runCommand(bridge: CloudflareBridge, id: string, command: string, cwd: string, signal?: AbortSignal) {
  const response = await bridge.exec(id, command, cwd, signal);
  if (!response.body) throw new Error("Sandbox command returned no stream");
  const decoder = new TextDecoder();
  let buffer = "";
  const stdoutChunks: Buffer[] = [];
  const stderrChunks: Buffer[] = [];
  let stdoutBytes = 0;
  let stderrBytes = 0;
  let exitCode: number | null = null;
  for await (const bytes of response.body) {
    buffer = (buffer + decoder.decode(bytes, { stream: true })).replaceAll("\r\n", "\n");
    if (buffer.length > MAX_OUTPUT_BYTES * 3) throw new Error("Sandbox command output exceeded limit");
    for (let end = buffer.indexOf("\n\n"); end >= 0; end = buffer.indexOf("\n\n")) {
      const frame = buffer.slice(0, end).replaceAll("\r", "");
      buffer = buffer.slice(end + 2);
      const event = /^event: ([^\n]+)/m.exec(frame)?.[1];
      const data = /^data: (.*)$/m.exec(frame)?.[1];
      if (event === "stdout" || event === "stderr") {
        if (data === undefined) throw new Error("Sandbox output frame is malformed");
        if (!/^(?:[a-zA-Z0-9+/]{4})*(?:[a-zA-Z0-9+/]{2}==|[a-zA-Z0-9+/]{3}=)?$/.test(data))
          throw new Error("Sandbox output encoding is invalid");
        const chunk = Buffer.from(data, "base64");
        if (event === "stdout") { stdoutChunks.push(chunk); stdoutBytes += chunk.byteLength; }
        else { stderrChunks.push(chunk); stderrBytes += chunk.byteLength; }
        if (stdoutBytes > MAX_OUTPUT_BYTES || stderrBytes > MAX_OUTPUT_BYTES) throw new Error("Sandbox output exceeded limit");
      } else if (event === "exit") {
        const decoded: unknown = JSON.parse(data ?? "null");
        exitCode = z.object({ exit_code: z.number().int() }).parse(decoded).exit_code;
      } else if (event === "error") {
        throw new Error("Sandbox command failed remotely");
      }
    }
  }
  if (exitCode === null) throw new Error("Sandbox command ended without confirmed exit status");
  return { stdout: Buffer.concat(stdoutChunks).toString("utf8"), stderr: Buffer.concat(stderrChunks).toString("utf8"), exitCode };
}

function session(bridge: CloudflareBridge, id: string, sessionKey: string): SandboxSession {
  async function read(options: { path: string; abortSignal?: AbortSignal }) {
    return bridge.read(id, options.path, options.abortSignal);
  }
  async function write(options: { path: string; content: Uint8Array; abortSignal?: AbortSignal }) {
    await bridge.write(id, options.path, options.content, options.abortSignal);
  }
  return {
    id: sessionKey,
    resolvePath,
    async setNetworkPolicy(policy) {
      if (policy !== "deny-all") throw new Error("Cloudflare sandbox outbound access cannot be enabled at runtime");
    },
    async run(options) {
      if (options.env && Object.keys(options.env).length) throw new Error("Sandbox per-command environment is forbidden");
      return runCommand(bridge, id, options.command, options.workingDirectory ?? "/workspace", options.abortSignal);
    },
    async spawn(options) {
      if (options.env && Object.keys(options.env).length) throw new Error("Sandbox per-command environment is forbidden");
      const result = runCommand(bridge, id, options.command, options.workingDirectory ?? "/workspace", options.abortSignal);
      const stdout = new ReadableStream<Uint8Array>({ async start(controller) {
        try { controller.enqueue(Buffer.from((await result).stdout)); controller.close(); } catch (error) { controller.error(error); }
      } });
      const stderr = new ReadableStream<Uint8Array>({ async start(controller) {
        try { controller.enqueue(Buffer.from((await result).stderr)); controller.close(); } catch (error) { controller.error(error); }
      } });
      return {
        stdout, stderr,
        async wait() { return { exitCode: (await result).exitCode }; },
        async kill() { await bridge.destroy(id); },
      };
    },
    async readFile(options) {
      const bytes = await read(options);
      return bytes === null ? null : new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(bytes); controller.close(); } });
    },
    readBinaryFile: read,
    async readTextFile(options) {
      const bytes = await read(options);
      if (bytes === null) return null;
      const text = bytes.toString((options.encoding ?? "utf8") as BufferEncoding);
      if (options.startLine === undefined && options.endLine === undefined) return text;
      if ((options.startLine !== undefined && (!Number.isSafeInteger(options.startLine) || options.startLine < 1)) ||
        (options.endLine !== undefined && (!Number.isSafeInteger(options.endLine) || options.endLine < 1)) ||
        (options.startLine ?? 1) > (options.endLine ?? Infinity)) throw new Error("Invalid sandbox line range");
      return (text.match(/[^\n]*\n|[^\n]+$/g) ?? []).slice((options.startLine ?? 1) - 1, options.endLine).join("");
    },
    async writeFile(options) {
      const chunks: Uint8Array[] = [];
      let size = 0;
      for await (const chunk of options.content) {
        size += chunk.byteLength;
        if (size > MAX_FILE_BYTES) throw new Error("Sandbox file exceeds size limit");
        chunks.push(chunk);
      }
      await write({ ...options, content: Buffer.concat(chunks) });
    },
    writeBinaryFile: write,
    async writeTextFile(options) { await write({ ...options, content: Buffer.from(options.content, options.encoding as BufferEncoding | undefined) }); },
    async removePath(options) {
      const filename = resolvePath(options.path);
      if (filename === "/workspace") throw new Error("Cannot remove workspace root");
      const flags = `${options.force ? "f" : ""}${options.recursive ? "r" : ""}`;
      const result = await runCommand(bridge, id, `rm ${flags ? `-${flags} ` : ""}-- ${shellQuote(filename)}`, "/workspace", options.abortSignal);
      if (result.exitCode !== 0) throw new Error("Sandbox file removal failed");
    },
  };
}

export function cloudflareSandbox(): SandboxBackend {
  const name = "sigiro-cloudflare";
  return {
    name,
    async prewarm(input) {
      if (input.bootstrap || input.seedFiles.length) throw new Error("Cloudflare sandbox templates cannot run bootstrap or seed files");
      return { reused: true };
    },
    async create(input) {
      const root = process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL;
      const key = process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY;
      const snapshotDirectory = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR;
      if (!root || !key || !snapshotDirectory || !path.isAbsolute(snapshotDirectory))
        throw new Error("Cloudflare sandbox host configuration is missing");
      const bridge = new CloudflareBridge(root, key);
      const encryptionKey = checkpointKey();
      await mkdir(snapshotDirectory, { recursive: true, mode: 0o700 });
      const snapshotInfo = await lstat(snapshotDirectory);
      if (!snapshotInfo.isDirectory() || snapshotInfo.mode & 0o077 ||
        (process.getuid && snapshotInfo.uid !== process.getuid()))
        throw new Error("Sandbox snapshot directory must be private and owned by this process");
      if (input.templateKey !== null) throw new Error("Cloudflare optimizer must start with an empty sandbox template");
      const digest = createHash("sha256").update(input.tags?.sessionId ?? input.sessionKey).digest("hex");
      const checkpoint = path.join(snapshotDirectory, `${digest}.tar`);
      const metadataFile = path.join(snapshotDirectory, `${digest}.json`);
      const previously = input.existingMetadata?.sandboxId;
      const originalId = previously === undefined ? null : Identifier.parse(previously);
      let id = originalId;
      const identityPath = "/workspace/.sigiro-sandbox-identity";
      const validIdentity = id !== null && await bridge.running(id) &&
        (await bridge.read(id, identityPath))?.toString("utf8") === input.sessionKey;
      if (id !== null && !validIdentity) {
        const archive = await readFile(checkpoint).catch((error: unknown) => {
          if (error instanceof Error && "code" in error && error.code === "ENOENT") return null;
          throw error;
        });
        if (!archive) throw new Error("Sandbox stopped without a recoverable workspace checkpoint");
        if (archive.byteLength > MAX_ARCHIVE_BYTES + 64) throw new Error("Encrypted sandbox checkpoint exceeds size limit");
        const restored = decryptCheckpoint(archive, input.sessionKey, encryptionKey);
        try {
          await bridge.hydrate(id, restored);
        } catch (error) {
          if (!(error instanceof Error && error.message.includes("rejected (404)"))) throw error;
          id = await bridge.create();
          try { await bridge.hydrate(id, restored); }
          catch (createError) { await bridge.destroy(id).catch(() => {}); throw createError; }
        }
        if ((await bridge.read(id, identityPath))?.toString("utf8") !== input.sessionKey)
          throw new Error("Sandbox checkpoint did not restore its session identity");
      }
      if (id === null) {
        id = await bridge.create();
        try {
          await bridge.write(id, identityPath, Buffer.from(input.sessionKey));
          const record = `${metadataFile}.${randomUUID()}.tmp`;
          await writeFile(record, JSON.stringify({ sandboxId: id }), { mode: 0o600, flag: "wx", flush: true });
          await rename(record, metadataFile);
        } catch (error) { await bridge.destroy(id).catch(() => {}); throw error; }
      }
      const sandboxId = id;
      const active = session(bridge, sandboxId, input.sessionKey);
      return {
        session: active,
        useSessionFn: async () => active,
        async captureState() {
          const archive = await bridge.persist(sandboxId);
          await mkdir(snapshotDirectory, { recursive: true, mode: 0o700 });
          const temporary = `${checkpoint}.${randomUUID()}.tmp`;
          await writeFile(temporary, encryptCheckpoint(archive, input.sessionKey, encryptionKey), { mode: 0o600, flag: "wx", flush: true });
          await rename(temporary, checkpoint);
          const record = `${metadataFile}.${randomUUID()}.tmp`;
          await writeFile(record, JSON.stringify({ sandboxId }), { mode: 0o600, flag: "wx", flush: true });
          await rename(record, metadataFile);
          const dir = await open(snapshotDirectory, "r");
          try { await dir.sync(); }
          finally { await dir.close(); }
          return { backendName: name, sessionKey: input.sessionKey, metadata: { sandboxId } };
        },
        async shutdown() { await bridge.destroy(sandboxId); },
      };
    },
  };
}

export async function cleanupCloudflareSandbox(sessionId: string): Promise<void> {
  const root = process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL;
  const key = process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY;
  const directory = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR;
  if (!root || !key || !directory || !path.isAbsolute(directory)) throw new Error("Sandbox cleanup is not configured");
  const digest = createHash("sha256").update(sessionId).digest("hex");
  const metadataFile = path.join(directory, `${digest}.json`);
  let record: Buffer;
  try { record = await readFile(metadataFile); }
  catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") return;
    throw error;
  }
  const payload: unknown = JSON.parse(record.toString("utf8"));
  const sandboxId = Identifier.parse(z.object({ sandboxId: z.string() }).parse(payload).sandboxId);
  await new CloudflareBridge(root, key).destroy(sandboxId);
  await rm(path.join(directory, `${digest}.tar`), { force: true });
  await rm(metadataFile, { force: true });
}
