import path from "node:path";
import type { SandboxSession } from "eve/sandbox";
import type { Change } from "./github.ts";

type File = { path: string; content: Uint8Array; executable: boolean; symlink?: boolean };
type WorkspaceIO = Pick<SandboxSession, "run" | "readBinaryFile" | "writeBinaryFile" | "writeTextFile">;
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
const absolute = (root: string, relative: string) => `${root}/${relative}`;

export function safePath(value: string) {
  return value.length > 0 && value.length <= 512 && !value.startsWith("/") && !value.includes("\0") &&
    value.split("/").every((part) => part !== "" && part !== "." && part !== ".." && part !== ".git");
}

export async function provisionWorkspace(sandbox: WorkspaceIO, files: readonly File[], root = "/workspace/repo"): Promise<void> {
  if (path.posix.basename(root) !== "repo" || !root.startsWith("/") || !files.length || files.length > 1024 ||
    files.some((file) => !safePath(file.path) || (file.symlink !== undefined && file.symlink !== true)) ||
    new Set(files.map((file) => file.path)).size !== files.length) throw new Error("Invalid repository snapshot or workspace");
  const total = files.reduce((sum, file) => sum + file.content.byteLength, 0);
  if (total > 20 * 1024 * 1024) throw new Error("Repository snapshot exceeds 20 MiB");
  const links = files.filter((file) => file.symlink).map((file) => {
    const target = new TextDecoder("utf-8", { fatal: true }).decode(file.content);
    const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(file.path), target));
    if (!/^[a-zA-Z0-9._/-]{1,512}$/.test(target) || target.startsWith("/") ||
      resolved === ".." || resolved.startsWith("../") || resolved.startsWith("/") ||
      resolved === file.path || resolved.startsWith(`${file.path}/`) || file.path.startsWith(`${resolved}/`)) {
      throw new Error("Unsafe or unresolved repository symlink");
    }
    return { file, target, resolved };
  });
  for (const link of links) {
    if (files.some((entry) => entry.path.startsWith(`${link.file.path}/`)) ||
      links.some((other) => other.file.path !== link.file.path &&
        (link.resolved === other.file.path || link.resolved.startsWith(`${other.file.path}/`) ||
          link.file.path.startsWith(`${other.file.path}/`)))) {
      throw new Error("Repository symlinks must not overlap or chain");
    }
  }
  const marker = `${path.posix.dirname(root)}/.sigiro-baseline-ready`;
  const existing = await sandbox.run({ command: `test -f ${quote(marker)}` });
  if (existing.exitCode === 0) return;
  const init = await sandbox.run({ command: `rm -rf -- ${quote(root)} && mkdir -p -- ${quote(root)} && git init -q -- ${quote(root)}` });
  if (init.exitCode !== 0) throw new Error("Unable to initialize credential-free workspace");
  for (const file of files.filter((entry) => !entry.symlink)) {
    const name = absolute(root, file.path);
    const mkdir = await sandbox.run({ command: `mkdir -p -- ${quote(path.posix.dirname(name))}` });
    if (mkdir.exitCode !== 0) throw new Error("Unable to create workspace path");
    await sandbox.writeBinaryFile({ path: name, content: file.content });
    const own = await sandbox.run({ command: `tmp=$(mktemp ${quote(`${name}.sigiro.XXXXXX`)}) && cp -- ${quote(name)} "$tmp" && mv -f -- "$tmp" ${quote(name)}` });
    if (own.exitCode !== 0) throw new Error("Unable to transfer file to sandbox user");
    if (file.executable) {
      const chmod = await sandbox.run({ command: `chmod 755 ${quote(name)}` });
      if (chmod.exitCode !== 0) throw new Error("Unable to restore executable mode");
    }
  }
  for (const { file, target } of links) {
    const name = absolute(root, file.path);
    const linked = await sandbox.run({ command: `mkdir -p -- ${quote(path.posix.dirname(name))} && ln -s -- ${quote(target)} ${quote(name)}` });
    if (linked.exitCode !== 0) throw new Error("Unable to create confined repository symlink");
  }
  const baseline = await sandbox.run({ command: `git -C ${quote(root)} add -A && git -C ${quote(root)} -c user.name=Sigiro -c user.email=optimizer@sigiro.com commit -q -m baseline` });
  if (baseline.exitCode !== 0) throw new Error("Unable to record local baseline");
  await sandbox.writeTextFile({ path: marker, content: "ready\n" });
}

export async function collectChanges(sandbox: WorkspaceIO, root = "/workspace/repo"): Promise<readonly Change[]> {
  if (path.posix.basename(root) !== "repo" || !root.startsWith("/")) throw new Error("Invalid workspace");
  const tracked = await sandbox.run({ command: `git -C ${quote(root)} diff --name-only --no-renames -z HEAD` });
  const untracked = await sandbox.run({ command: `git -C ${quote(root)} ls-files --others --exclude-standard -z` });
  if (tracked.exitCode !== 0 || untracked.exitCode !== 0) throw new Error("Unable to inspect workspace changes");
  const paths = [...new Set((tracked.stdout + untracked.stdout).split("\0").filter(Boolean))];
  if (!paths.length || paths.length > 100 || paths.some((file) => !safePath(file))) {
    throw new Error("No changes, too many changes or unsafe paths");
  }
  const changes: Change[] = [];
  let total = 0;
  for (const file of paths) {
    const original = await sandbox.run({ command: `git -C ${quote(root)} ls-files --stage -z -- ${quote(`:(literal)${file}`)}` });
    if (original.exitCode !== 0 || original.stdout.startsWith("120000 ")) throw new Error("Baseline symlinks cannot be modified");
    const name = quote(absolute(root, file));
    const kind = await sandbox.run({ command: `if test -L ${name}; then echo symlink; elif test -f ${name}; then echo file; elif test ! -e ${name}; then echo deleted; else echo unsupported; fi` });
    if (kind.exitCode !== 0 || !["file", "deleted"].includes(kind.stdout.trim())) {
      throw new Error("Unsupported changed file type");
    }
    if (kind.stdout.trim() === "deleted") {
      changes.push({ path: file, content: null });
      continue;
    }
    const bytes = await sandbox.readBinaryFile({ path: absolute(root, file) });
    if (bytes === null) throw new Error("Changed file disappeared during patch collection");
    total += bytes.byteLength;
    if (bytes.byteLength > 1024 * 1024 || total > 10 * 1024 * 1024) throw new Error("Patch exceeds size limit");
    const content = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    const mode = await sandbox.run({ command: `test -x ${name}` });
    changes.push({ path: file, content, executable: mode.exitCode === 0 });
  }
  return changes;
}
