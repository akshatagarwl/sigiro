#!/usr/bin/env node
// Copies the root package.json version (the one Changesets bumps) into the
// `sigiro` crate: Cargo.toml's [package] version and its Cargo.lock entry.
// release.yml refuses a tag that does not match Cargo.toml, so the two must
// never drift. `--check` only reports drift and exits non-zero.
//
// Edits the files as text instead of running cargo, so the release step
// needs no Rust toolchain. Only the `sigiro` crate is released; the
// workspace crates under crates/ keep their own versions.
import { readFileSync, writeFileSync } from "node:fs";

const check = process.argv.includes("--check");
const { version } = JSON.parse(readFileSync("package.json", "utf8"));
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`package.json version "${version}" is not a release version`);
}

// The first `version = "…"` after `[package]`, before the next table.
const replaceCargoToml = (text) => {
  const pattern = /(\[package\]\n(?:(?!\[)[^\n]*\n)*?version = ")([^"]+)(")/;
  const match = text.match(pattern);
  if (!match) throw new Error("Cargo.toml: no [package] version");
  return { current: match[2], next: text.replace(pattern, `$1${version}$3`) };
};

// The `[[package]]` entry whose name is exactly "sigiro".
const replaceCargoLock = (text) => {
  const pattern = /(\[\[package\]\]\nname = "sigiro"\nversion = ")([^"]+)(")/;
  const match = text.match(pattern);
  if (!match) throw new Error('Cargo.lock: no [[package]] named "sigiro"');
  return { current: match[2], next: text.replace(pattern, `$1${version}$3`) };
};

let drift = false;
for (const [file, replace] of [
  ["Cargo.toml", replaceCargoToml],
  ["Cargo.lock", replaceCargoLock],
]) {
  const text = readFileSync(file, "utf8");
  const { current, next } = replace(text);
  if (current === version) continue;
  drift = true;
  if (check) {
    console.error(`${file} has ${current}, package.json has ${version}`);
  } else {
    writeFileSync(file, next);
    console.log(`${file}: ${current} -> ${version}`);
  }
}
if (check && drift) process.exit(1);
