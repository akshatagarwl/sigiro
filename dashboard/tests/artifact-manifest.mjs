import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const dist = path.join(root, "dashboard/dist");
const sha256 = (value) => createHash("sha256").update(value).digest("hex");
const git = (...args) =>
  execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
const sourcePaths = [
  ...new Set(
    git(
      "ls-files",
      "-co",
      "--exclude-standard",
      "-z",
      "--",
      "dashboard",
      "package.json",
      "aube-lock.yaml"
    )
      .split("\0")
      .filter(Boolean)
  ),
].sort();
const treeHash = (entries) =>
  sha256(entries.map((entry) => `${entry.path}\0${entry.sha256}\n`).join(""));
const source = sourcePaths.map((name) => ({
  path: name,
  sha256: sha256(fs.readFileSync(path.join(root, name))),
}));
function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const name = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(name) : [name];
  });
}
const files = walk(dist)
  .sort()
  .map((name) => ({
    path: path.relative(dist, name),
    bytes: fs.statSync(name).size,
    sha256: sha256(fs.readFileSync(name)),
  }));
const manifest = {
  createdAt: new Date().toISOString(),
  gitHeadAtManifestTime: git("rev-parse", "HEAD"),
  sourceTreeSha256: treeHash(source),
  sourceFileCount: source.length,
  sourceScope:
    "Tracked and non-ignored untracked dashboard files plus root package.json/aube-lock.yaml; excludes build output, node_modules and docs. Digest: SHA256 of sorted path + NUL + file SHA256 + newline.",
  artifactTreeSha256: treeHash(files),
  fileCount: files.length,
  files,
};
const destination =
  process.argv[2] ?? "/tmp/sigiro-dashboard-evidence/artifact-manifest.json";
fs.mkdirSync(path.dirname(destination), { recursive: true });
fs.writeFileSync(destination, JSON.stringify(manifest, null, 2) + "\n");
console.log(
  JSON.stringify(
    {
      sourceTreeSha256: manifest.sourceTreeSha256,
      artifactTreeSha256: manifest.artifactTreeSha256,
      fileCount: manifest.fileCount,
      destination,
    },
    null,
    2
  )
);
