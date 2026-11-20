// aube-lock.yaml -> pnpm-lock.yaml, for nix/aube-project.nix.
//
// aube writes pnpm's lockfile v9 format, with differences pnpm 10 rejects:
//
// 1. Patches. `patchedDependencies` entries record only the patch hash; pnpm
//    expects `{ hash, path }`. The hash is the same (sha256 of the patch
//    file), so the path is added from package.json `aube.patchedDependencies`.
//
// 2. npm aliases (`"x": "npm:y@1.0.0"`). aube keeps a package and snapshot
//    `x@1.0.0` marked `aliasOf: y` and references it as `x: 1.0.0`. pnpm has
//    only `y@1.0.0` and references it as `x: y@1.0.0`.
//
// 3. Dangling peer variants. aube 2.8 can write a snapshot whose dependency
//    line names a peer variant that has no snapshot of its own, while the
//    snapshot's own key names the variant that does exist (www/:
//    @alchemy.run/frontend-frameworks -> @vitejs/plugin-react, vite with
//    @types/node 24 vs 22). aube ignores it; pnpm fails with
//    ERR_PNPM_LOCKFILE_MISSING_DEPENDENCY. The key is authoritative, so the
//    dependency is pointed at the variant the key names.
//
// Every line not rewritten is copied byte for byte, every repair is reported
// on stderr, and anything that does not fit these cases fails the build
// instead of guessing. pnpm then installs the result with --frozen-lockfile.
//
// Usage: node aube-to-pnpm-lock.mjs aube-lock.yaml package.json > pnpm-lock.yaml

import { readFileSync } from "node:fs";

const [lockPath, packagePath] = process.argv.slice(2);
const text = readFileSync(lockPath, "utf8");
const patches = JSON.parse(readFileSync(packagePath, "utf8")).aube?.patchedDependencies ?? {};

const fail = (message) => {
  process.stderr.write(`aube-to-pnpm-lock: ${message}\n`);
  process.exit(1);
};
const note = (message) => process.stderr.write(`aube-to-pnpm-lock: ${message}\n`);
const unquote = (s) => s.replace(/^'(.*)'$/, "$1").replace(/^"(.*)"$/, "$1");
// pnpm's style: quote only what YAML would misread (a leading `@`).
const quote = (s) => (/^[A-Za-z0-9]/.test(s) ? s : `'${s}'`);

// --- parse: top-level sections, each a list of 2-space-indented entries ---

const lines = text.split("\n");
const sections = []; // { header, preamble: [], entries: [{ key, keyLine, body: [] }] }
for (const line of lines) {
  if (/^\S/.test(line)) {
    sections.push({ header: line, name: line.replace(/:.*/, ""), preamble: [], entries: [] });
    continue;
  }
  const section = sections.at(-1);
  if (!section) fail("lockfile does not start with a top-level key");
  const entry = /^  (\S.*?):(?: (.*))?$/.exec(line);
  if (entry) {
    section.entries.push({ key: unquote(entry[1]), inline: entry[2], keyLine: line, body: [] });
  } else if (section.entries.length) {
    section.entries.at(-1).body.push(line);
  } else {
    section.preamble.push(line);
  }
}
const section = (name) => sections.find((s) => s.name === name);
const packages = section("packages")?.entries ?? [];
const snapshots = section("snapshots")?.entries ?? [];

// `name@version(peers...)` -> [name, version-with-peers]
const splitRef = (ref) => {
  const at = ref.indexOf("@", 1);
  return [ref.slice(0, at), ref.slice(at + 1)];
};
const baseVersion = (version) => version.replace(/\(.*$/, "");
const isAliasedVersion = (version) => /^@?[^@(]+@/.test(version) && !/^\d/.test(version);

// --- 1. patches ---

for (const entry of section("patchedDependencies")?.entries ?? []) {
  if (!/^[0-9a-f]{64}$/.test(entry.inline ?? "")) continue; // already pnpm's shape
  if (!(entry.key in patches)) fail(`${entry.key} is patched in the lockfile but not in package.json`);
  entry.keyLine = `  '${entry.key}':`;
  entry.body = [`    hash: ${entry.inline}`, `    path: ${patches[entry.key]}`, ...entry.body];
}

// --- 3. dangling peer variants (before aliases: uses aube's own keys) ---

const snapshotKeys = new Set(snapshots.map((e) => e.key));
// The `(name@...)` group for `name` in a snapshot key, parentheses balanced.
const peerFromKey = (key, name) => {
  const start = key.indexOf(`(${name}@`);
  if (start < 0) return null;
  for (let i = start, depth = 0; i < key.length; i++) {
    if (key[i] === "(") depth++;
    if (key[i] === ")" && --depth === 0) return key.slice(start + 1, i);
  }
  return null;
};
const depLine = /^      (\S+): (\S+)$/;
for (const entry of snapshots) {
  entry.body = entry.body.map((line) => {
    const m = depLine.exec(line);
    if (!m) return line;
    const name = unquote(m[1]);
    const version = unquote(m[2]);
    if (isAliasedVersion(version) || version.startsWith("link:") || snapshotKeys.has(`${name}@${version}`)) return line;
    const fromKey = peerFromKey(entry.key, name);
    if (!fromKey || !snapshotKeys.has(fromKey)) fail(`${entry.key}: ${name}@${version} has no snapshot, and the key names no usable variant`);
    note(`repaired a dangling ${name} reference in ${entry.key.slice(0, 50)}...`);
    return `      ${m[1]}: ${quote(splitRef(fromKey)[1])}`;
  });
}

// --- 2. aliases ---

// "x@1.0.0" -> "y"
const aliases = new Map();
for (const entry of packages) {
  const line = entry.body.find((l) => /^    aliasOf: /.test(l));
  if (line) aliases.set(entry.key, unquote(line.replace(/^    aliasOf: /, "")));
}
if (aliases.size) {
  const packageKeys = new Set(packages.map((e) => e.key));
  const target = (name, version) => {
    const of = aliases.get(`${name}@${baseVersion(version)}`);
    return of === undefined ? null : `${of}@${version}`;
  };

  // Packages: drop the alias entry when the target exists, else rename it.
  section("packages").entries = packages.flatMap((entry) => {
    if (!aliases.has(entry.key)) return [entry];
    const [, version] = splitRef(entry.key);
    const to = `${aliases.get(entry.key)}@${version}`;
    note(`alias ${entry.key} -> ${to}`);
    if (packageKeys.has(to)) return [];
    packageKeys.add(to);
    return [{ ...entry, keyLine: `  ${quote(to)}:`, body: entry.body.filter((l) => !/^    aliasOf: /.test(l)) }];
  });

  // Snapshots: the same, for every peer variant of an alias.
  const keys = new Set(snapshots.map((e) => e.key));
  section("snapshots").entries = snapshots.flatMap((entry) => {
    const [name, version] = splitRef(entry.key);
    const to = target(name, version);
    if (to === null) return [entry];
    if (keys.has(to)) return [];
    keys.add(to);
    return [{ ...entry, keyLine: `  ${quote(to)}:${entry.inline !== undefined ? ` ${entry.inline}` : ""}` }];
  });

  // References from snapshots: `x: 1.0.0` -> `x: y@1.0.0`.
  for (const entry of section("snapshots").entries) {
    entry.body = entry.body.map((line) => {
      const m = depLine.exec(line);
      if (!m) return line;
      const to = target(unquote(m[1]), unquote(m[2]));
      return to === null ? line : `      ${m[1]}: ${quote(to)}`;
    });
  }

  // References from importers: `version: 1.0.0` under dependency `x`.
  for (const importer of section("importers")?.entries ?? []) {
    let dep = null;
    importer.body = importer.body.map((line) => {
      const name = /^      (\S+):$/.exec(line);
      if (name) dep = unquote(name[1]);
      const version = /^        version: (\S+)$/.exec(line);
      if (!version || dep === null) return line;
      const to = target(dep, unquote(version[1]));
      return to === null ? line : `        version: ${quote(to)}`;
    });
  }
}

// --- emit ---

const out = [];
for (const s of sections) {
  out.push(s.header, ...s.preamble);
  for (const e of s.entries) out.push(e.keyLine, ...e.body);
}
const result = out.join("\n");
if (/^    aliasOf: /m.test(result)) fail("an aliasOf entry survived the translation");
process.stdout.write(result);
