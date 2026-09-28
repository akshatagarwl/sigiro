import assert from "node:assert/strict";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { mkdtemp, readFile, rm, rmdir } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { cleanupCloudflareSandbox, cloudflareSandbox } from "./cloudflare.ts";
import { collectChanges, provisionWorkspace } from "./workspace.ts";

if (!process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_URL || !process.env.SIGIRO_CLOUDFLARE_SANDBOX_API_KEY) {
  throw new Error("Live Cloudflare bridge URL and key are required");
}
const directory = await mkdtemp(path.join(os.tmpdir(), "sigiro-cloudflare-live-"));
const originalEncryptionKey = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = randomBytes(32).toString("hex");
process.env.SIGIRO_OPTIMIZER_SNAPSHOT_DIR = directory;
const backend = cloudflareSandbox();
const sessionId = randomUUID();
const sessionKey = `eve-sbx-ses-${sessionId}-root`;
const input = { templateKey: null, sessionKey, tags: { sessionId, agent: "optimizer", channel: "eve" }, runtimeContext: { appRoot: process.cwd() } };
let initial: Awaited<ReturnType<typeof backend.create>> | undefined;
let resumed: Awaited<ReturnType<typeof backend.create>> | undefined;
try {
  initial = await backend.create(input);
  await provisionWorkspace(initial.session, [
    { path: "checkout.txt", content: Buffer.from("slow\n"), executable: false },
    { path: "validate.sh", content: Buffer.from("#!/bin/sh\ngrep -q optimized checkout.txt\n"), executable: true },
    { path: "AGENTS.md", content: Buffer.from("Validate offline.\n"), executable: false },
    { path: "CLAUDE.md", content: Buffer.from("AGENTS.md"), executable: false, symlink: true },
  ]);
  assert.equal((await initial.session.run({ command: "test -L /workspace/repo/CLAUDE.md && test \"$(readlink /workspace/repo/CLAUDE.md)\" = AGENTS.md" })).exitCode, 0);
  assert.notEqual((await initial.session.run({ command: "cd /workspace/repo && ./validate.sh" })).exitCode, 0);
  await initial.session.writeTextFile({ path: "/workspace/repo/checkout.txt", content: "optimized\n" });
  assert.equal((await initial.session.run({ command: "cd /workspace/repo && ./validate.sh" })).exitCode, 0);
  const expected = await collectChanges(initial.session);
  assert.deepEqual(expected.map((change) => change.path), ["checkout.txt"]);
  assert.equal(expected[0].content, "optimized\n");
  assert.equal((await initial.session.run({ command: "node --version" })).exitCode, 0);
  const networkNamespace = await initial.session.run({ command: 'test "$(readlink /proc/self/ns/net)" != "$(readlink /proc/1/ns/net)"' });
  assert.equal(networkNamespace.exitCode, 0, "Commands must run in a separate network namespace");
  assert.equal((await initial.session.run({ command: "command -v nsenter >/dev/null" })).exitCode, 0);
  const escape = await initial.session.run({ command: "nsenter --net=/proc/1/ns/net -- true" });
  assert.notEqual(escape.exitCode, 0, "Untrusted commands must not rejoin the Sandbox control network namespace");
  const dns = await initial.session.run({ command: "getent hosts example.com" });
  assert.notEqual(dns.exitCode, 0, "Untrusted commands must not resolve external DNS");
  const direct = await initial.session.run({ command: "python3 -c 'import socket; socket.create_connection((\"1.1.1.1\", 53), timeout=3)'" });
  assert.notEqual(direct.exitCode, 0, "Untrusted commands must not bypass DNS using a resolver IP");
  const isolated = await initial.session.run({ command: "test -z \"${SIGIRO_CLOUDFLARE_SANDBOX_API_KEY:-}\" && test -z \"${SIGIRO_GITHUB_APP_PRIVATE_KEY:-}\" && test -z \"${SIGIRO_OPTIMIZER_INTERNAL_TOKEN:-}\"" });
  assert.equal(isolated.exitCode, 0);
  const external = await initial.session.run({ command: "node -e 'fetch(\"https://example.com\",{signal:AbortSignal.timeout(4000)}).then(()=>process.exit(0),()=>process.exit(1))'" });
  assert.notEqual(external.exitCode, 0);
  const state = await initial.captureState();
  const sealed = await readFile(path.join(directory, `${createHash("sha256").update(sessionId).digest("hex")}.tar`));
  assert.equal(sealed.subarray(0, 12).toString(), "SIGIRO-CF-1\0");
  assert.equal(sealed.includes(Buffer.from("optimized\n")), false);
  await initial.shutdown();
  initial = undefined;
  resumed = await backend.create({ ...input, existingMetadata: state.metadata });
  assert.equal(await resumed.session.readTextFile({ path: "repo/checkout.txt" }), "optimized\n");
  assert.equal((await resumed.session.run({ command: "test -L /workspace/repo/CLAUDE.md && test \"$(readlink /workspace/repo/CLAUDE.md)\" = AGENTS.md" })).exitCode, 0);
  assert.deepEqual(await collectChanges(resumed.session), expected);
  await cleanupCloudflareSandbox(sessionId);
  resumed = undefined;
  console.log("Cloudflare sandbox: validation, namespace-isolated DNS and HTTP, restored Git diff passed");
} finally {
  if (initial) await initial.shutdown();
  if (resumed) await resumed.shutdown();
  const digest = createHash("sha256").update(sessionId).digest("hex");
  await rm(path.join(directory, `${digest}.tar`), { force: true });
  await rm(path.join(directory, `${digest}.json`), { force: true });
  await rmdir(directory);
  if (originalEncryptionKey === undefined) delete process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
  else process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = originalEncryptionKey;
}
