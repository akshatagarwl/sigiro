import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, readlink, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { collectChanges, provisionWorkspace } from "./workspace.ts";

const dir = await mkdtemp(path.join(os.tmpdir(), "sigiro-optimizer-git-"));
const source = path.join(dir, "source");
const destination = path.join(dir, "vm", "repo");
const git = (args: string[]) => execFileSync("git", ["-C", source, ...args], { encoding: "utf8" }).trim();
try {
  await mkdir(source);
  execFileSync("git", ["init", "-q", source]);
  await writeFile(path.join(source, "checkout.txt"), "old latency\n");
  await writeFile(path.join(source, "remove.txt"), "obsolete\n");
  await writeFile(path.join(source, "validate.sh"), "#!/bin/sh\ntest -f checkout.txt\n", { mode: 0o755 });
  await writeFile(path.join(source, "AGENTS.md"), "Keep validations offline.\n");
  await mkdir(path.join(source, ".agents", "skills"), { recursive: true });
  await mkdir(path.join(source, ".claude"));
  await mkdir(path.join(source, ".opencode", "references"), { recursive: true });
  await writeFile(path.join(source, ".agents", "skills", "safe.md"), "A local skill.\n");
  await symlink("AGENTS.md", path.join(source, "CLAUDE.md"));
  await symlink("../.agents/skills", path.join(source, ".claude", "skills"));
  await symlink("../../.agents/references/effect-smol", path.join(source, ".opencode", "references", "effect-smol"));
  git(["add", "-A"]);
  git(["-c", "user.name=Test", "-c", "user.email=test@local", "commit", "-qm", "baseline"]);
  const entries = git(["ls-tree", "-r", "--name-only", "HEAD"]).split("\n");
  const files = await Promise.all(entries.map(async (file) => {
    const mode = git(["ls-files", "-s", file]);
    const link = mode.startsWith("120000");
    return { path: file, content: link ? Buffer.from(await readlink(path.join(source, file))) : await readFile(path.join(source, file)),
      executable: mode.startsWith("100755"), ...(link ? { symlink: true } : {}) };
  }));
  const sandbox = {
    async run({ command }: { command: string }) {
      const result = spawnSync("/bin/bash", ["-lc", command], { encoding: "utf8", maxBuffer: 2 * 1024 * 1024 });
      return { exitCode: result.status ?? 1, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
    },
    async writeBinaryFile({ path: file, content }: { path: string; content: Uint8Array }) {
      await writeFile(file, content);
    },
    async writeTextFile({ path: file, content }: { path: string; content: string }) {
      await writeFile(file, content);
    },
    async readBinaryFile({ path: file }: { path: string }) {
      return readFile(file);
    },
  };
  await provisionWorkspace(sandbox, files, destination);
  assert.equal(await readlink(path.join(destination, "CLAUDE.md")), "AGENTS.md");
  assert.equal(await readlink(path.join(destination, ".claude", "skills")), "../.agents/skills");
  assert.equal(await readlink(path.join(destination, ".opencode", "references", "effect-smol")), "../../.agents/references/effect-smol");
  assert.equal((await sandbox.run({ command: `cd '${destination}' && ./validate.sh` })).exitCode, 0);
  const checkoutBoundary = Array.from({ length: 643 }, (_, i) => ({ path: `fixture/${i}.txt`, content: Buffer.from("x"), executable: false }));
  await provisionWorkspace({ ...sandbox, run: async () => ({ exitCode: 0, stdout: "", stderr: "" }) }, checkoutBoundary, destination);
  await assert.rejects(provisionWorkspace(sandbox, [...checkoutBoundary, ...Array.from({ length: 382 }, (_, i) => ({ path: `extra/${i}`, content: Buffer.from("x"), executable: false }))], destination));
  for (const target of ["../../etc/passwd", "/etc/passwd", "link/deeper", "../CLAUDE.md"]) {
    await assert.rejects(provisionWorkspace(sandbox,
      [...files, { path: "unsafe/link", content: Buffer.from(target), executable: false, symlink: true }], destination));
  }
  await writeFile(path.join(destination, "checkout.txt"), "less latency\n");
  await rm(path.join(destination, "remove.txt"));
  await writeFile(path.join(destination, "new.txt"), "evidence\n");
  const changes = await collectChanges(sandbox, destination);
  assert.deepEqual(changes.map(({path}) => path).sort(), ["checkout.txt", "new.txt", "remove.txt"]);
  assert.equal(changes.find(({path}) => path === "remove.txt")?.content, null);
  assert.equal(changes.find(({path}) => path === "checkout.txt")?.content, "less latency\n");
  await rm(path.join(destination, "CLAUDE.md"));
  await assert.rejects(collectChanges(sandbox, destination), /Baseline symlinks/);
  console.log("Bounded Git checkout, confined symlinks, native validation, edits and deletions passed");
} finally {
  await rm(dir, { recursive: true, force: true });
}
