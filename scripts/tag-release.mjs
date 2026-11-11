#!/usr/bin/env node
// Run on every push to main by .github/workflows/tag-on-main.yml. When origin
// has no `v<version>` tag for the version in package.json, creates an
// annotated tag at HEAD, pushes it, and writes `tag=v<version>` to
// $GITHUB_OUTPUT so the same run calls release.yml. When the tag exists it
// writes `tag=` and does nothing, so a version is tagged and released once.
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";

const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim();
const output = (value) => {
  if (process.env.GITHUB_OUTPUT)
    appendFileSync(process.env.GITHUB_OUTPUT, `tag=${value}\n`);
};

// Never tag a version Cargo.toml does not carry: release.yml would reject it.
execFileSync("node", ["scripts/sync-release-version.mjs", "--check"], {
  stdio: "inherit",
});

const { version } = JSON.parse(readFileSync("package.json", "utf8"));
const tag = `v${version}`;
if (git("ls-remote", "--tags", "origin", `refs/tags/${tag}`)) {
  console.log(`${tag} already exists on origin; nothing to tag or release`);
  output("");
} else {
  git("tag", "-a", tag, "-m", `sigiro ${version}`);
  git("push", "origin", `refs/tags/${tag}`);
  console.log(`pushed ${tag}`);
  output(tag);
}
