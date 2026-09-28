import assert from "node:assert/strict";
import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { cloudflareSandbox } from "./cloudflare.ts";

const directory = await mkdtemp(path.join(os.tmpdir(), "sigiro-cloudflare-"));
const key = "local-bridge-authentication-test-key-long-enough";
const sandboxes = new Map<string, string>();
let nextId = 0;
let transientHydrateError = true;
const server = createServer(async (request, response) => {
  if (request.headers.authorization !== `Bearer ${key}`) { response.writeHead(401).end(); return; }
  const url = new URL(request.url!, "http://127.0.0.1");
  if (request.method === "POST" && url.pathname === "/v1/sandbox") {
    const id = `sandbox-${++nextId}`;
    const workspace = path.join(directory, id);
    await mkdir(workspace, { recursive: true });
    sandboxes.set(id, workspace);
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify({ id }));
    return;
  }
  const [, , , id, operation, ...file] = url.pathname.split("/");
  if (operation === "running") {
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify({ running: sandboxes.has(id) }));
    return;
  }
  const workspace = sandboxes.get(id);
  if (!workspace) { response.writeHead(404).end(); return; }
  if (request.method === "DELETE" && !operation) {
    sandboxes.delete(id);
    await rm(workspace, { recursive: true, force: true });
    response.writeHead(204).end();
    return;
  }
  if (operation === "file") {
    const filename = path.resolve(workspace, file.slice(1).map(decodeURIComponent).join("/"));
    if (!filename.startsWith(`${workspace}/`)) { response.writeHead(403).end(); return; }
    if (request.method === "GET") {
      try { response.end(await readFile(filename)); }
      catch (error) { if (error instanceof Error && "code" in error && error.code === "ENOENT") response.writeHead(404).end(); else throw error; }
    } else if (request.method === "PUT") {
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.from(chunk));
      await mkdir(path.dirname(filename), { recursive: true });
      await writeFile(filename, Buffer.concat(chunks));
      response.end(JSON.stringify({ ok: true }));
    }
    return;
  }
  if (operation === "persist") {
    response.end(execFileSync("tar", ["-cf", "-", "-C", workspace, "."]));
    return;
  }
  if (operation === "hydrate") {
    if (transientHydrateError) {
      transientHydrateError = false;
      response.writeHead(503).end();
      return;
    }
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    const extracted = spawnSync("tar", ["-xf", "-", "-C", workspace], { input: Buffer.concat(chunks) });
    if (extracted.status !== 0) throw new Error("Local workspace restore failed");
    response.end(JSON.stringify({ ok: true }));
    return;
  }
  if (operation === "exec") {
    response.setHeader("Content-Type", "text/event-stream");
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    const body = JSON.parse(Buffer.concat(chunks).toString()) as { argv: string[]; cwd: string };
    // The local HTTP mock does not create a Linux namespace, but must reject
    // any optimizer command that bypasses the fail-closed production wrapper.
    assert.deepEqual(body.argv.slice(0, 7), ["unshare", "--user", "--map-root-user", "--net", "--", "sh", "-lc"]);
    const command = body.argv[7].replaceAll("/workspace", workspace);
    const cwd = body.cwd.replace("/workspace", workspace);
    const executed = spawnSync("sh", ["-lc", command], { cwd, encoding: "buffer", timeout: 10000 });
    response.write(`event: stdout\ndata: ${(executed.stdout ?? Buffer.alloc(0)).toString("base64")}\n\n`);
    response.write(`event: stderr\ndata: ${(executed.stderr ?? Buffer.alloc(0)).toString("base64")}\n\n`);
    response.end(`event: exit\ndata: ${JSON.stringify({ exit_code: executed.status ?? 1 })}\n\n`);
    return;
  }
  response.writeHead(404).end();
});
await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
if (!address || typeof address !== "object") throw new Error("Bridge mock has no port");
const original = {
  url: process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL,
  key: process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY,
  snapshots: process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR,
  encryptionKey: process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY,
};
process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = randomBytes(32).toString("hex");
process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL = `http://127.0.0.1:${address.port}`;
process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY = key;
process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR = directory;
try {
  const backend = cloudflareSandbox();
  await backend.prewarm({ templateKey: "unused", seedFiles: [], runtimeContext: { appRoot: directory } });
  const input = { templateKey: null, sessionKey: "session-a", runtimeContext: { appRoot: directory } };
  const first = await backend.create(input);
  await first.session.writeTextFile({ path: "/workspace/repo/main.py", content: "print('safe')\n" });
  const result = await first.session.run({ command: "printf ready" });
  assert.equal(result.exitCode, 0);
  assert.equal(result.stdout, "ready");
  assert.equal(await first.session.readTextFile({ path: "/workspace/repo/main.py" }), "print('safe')\n");
  await assert.rejects(Promise.resolve(first.session.readFile({ path: "/etc/passwd" })));
  await assert.rejects(first.session.setNetworkPolicy("allow-all"));
  const state = await first.captureState();
  const archive = await readFile(path.join(directory, `${(await import("node:crypto")).createHash("sha256").update("session-a").digest("hex")}.tar`));
  assert.ok(archive.byteLength > 0);
  assert.equal(archive.subarray(0, 12).toString(), "SIGIRO-CF-1\0");
  assert.equal(archive.includes(Buffer.from("print('safe')")), false);
  await first.shutdown();
  const file = path.join(directory, `${(await import("node:crypto")).createHash("sha256").update("session-a").digest("hex")}.tar`);
  const corrupt = Buffer.from(archive);
  corrupt[corrupt.length - 1] ^= 1;
  await writeFile(file, corrupt);
  await assert.rejects(backend.create({ ...input, existingMetadata: state.metadata }), /authentication failed/);
  await writeFile(file, archive);
  const resumed = await backend.create({ ...input, existingMetadata: state.metadata });
  assert.equal(await resumed.session.readTextFile({ path: "repo/main.py" }), "print('safe')\n");
  const process = await resumed.session.spawn({ command: "printf ready" });
  assert.equal((await process.wait()).exitCode, 0);
  assert.equal(await new Response(process.stdout).text(), "ready");
  await resumed.session.removePath({ path: "/workspace/repo/main.py" });
  assert.equal(await resumed.session.readFile({ path: "/workspace/repo/main.py" }), null);
  await resumed.shutdown();
  console.log("Authenticated Cloudflare bridge I/O, egress policy rejection, checkpoint, destroy and restore passed");
} finally {
  for (const [name, value] of Object.entries({
    SIGIRO_CLOUDFLARE_SANDBOX_API_URL: original.url,
    SIGIRO_CLOUDFLARE_SANDBOX_API_KEY: original.key,
    SIGIRO_OPTIMIZER_SNAPSHOT_DIR: original.snapshots,
    SIGIRO_OPTIMIZER_SNAPSHOT_KEY: original.encryptionKey,
  })) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await rm(directory, { recursive: true, force: true });
}
