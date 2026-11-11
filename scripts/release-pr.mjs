#!/usr/bin/env node
// Opens the release PR: `aube run release:pr`.
//
// From origin/main it runs `aube run release:version` (changeset version, then
// Cargo.toml/Cargo.lock sync), commits the result on release/v<version>, pushes
// it and opens a PR with `gh`. A person or agent opens that PR, so CI runs on
// it like any other. Merging it lets tag-on-main.yml tag and release.
//
// `--dry-run` makes the commit, prints it, then deletes the branch: nothing is
// pushed. Either way the checkout returns to the branch it started on.
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";

const dryRun = process.argv.includes("--dry-run");
const run = (cmd, args, opts = {}) =>
  execFileSync(cmd, args, { encoding: "utf8", ...opts });
const git = (...args) => run("git", args).trim();
const fail = (message) => {
  console.error(`release:pr: ${message}`);
  process.exit(1);
};

if (git("status", "--porcelain", "--untracked-files=no")) {
  fail("the working tree has uncommitted changes; commit or stash them first");
}
git("fetch", "--quiet", "origin", "main");
const pending = git("ls-tree", "--name-only", "origin/main", ".changeset/")
  .split("\n")
  .filter((path) => path.endsWith(".md") && !path.endsWith("/README.md"));
if (pending.length === 0)
  fail("origin/main has no pending changesets; nothing to release");

const start = git("rev-parse", "--abbrev-ref", "HEAD");
const startRef = start === "HEAD" ? git("rev-parse", "HEAD") : start;
let branch;
try {
  git("switch", "--quiet", "--detach", "origin/main");
  run("aube", ["run", "release:version"], { stdio: "inherit" });
  const { version } = JSON.parse(readFileSync("package.json", "utf8"));
  const tag = `v${version}`;
  branch = `release/${tag}`;
  if (git("ls-remote", "--heads", "origin", branch))
    fail(`origin already has ${branch}`);
  if (git("ls-remote", "--tags", "origin", tag))
    fail(`origin already has the tag ${tag}`);

  // The new section: from its heading to the next version heading.
  const changelog = readFileSync("CHANGELOG.md", "utf8");
  const from = changelog.indexOf(`## ${version}\n`);
  const to = changelog.indexOf("\n## ", from + 1);
  const notes = changelog.slice(from, to === -1 ? undefined : to).trim();

  git("switch", "--quiet", "-c", branch);
  const left = readdirSync(".changeset").filter(
    (f) => f.endsWith(".md") && f !== "README.md"
  );
  if (left.length > 0)
    fail(`changeset version left ${left.join(", ")} in .changeset/`);
  git(
    "add",
    "--all",
    "package.json",
    "Cargo.toml",
    "Cargo.lock",
    "CHANGELOG.md",
    ".changeset"
  );
  git("commit", "--quiet", "-m", `chore(release): sigiro ${tag}`, "-m", notes);
  console.log(git("show", "--stat", "--format=%h %s", "HEAD"));

  if (dryRun) {
    console.log(
      run("git", [
        "show",
        "--format=",
        "HEAD",
        "--",
        "package.json",
        "Cargo.toml",
        "CHANGELOG.md",
      ])
    );
    console.log(`release:pr: dry run, ${branch} not pushed`);
  } else {
    git("push", "--quiet", "-u", "origin", branch);
    const body = `${notes}\n\nMerging this PR tags \`${tag}\` on main and publishes the release (tag-on-main.yml → release.yml).`;
    const url = run("gh", [
      "pr",
      "create",
      "--base",
      "main",
      "--head",
      branch,
      "--title",
      `chore(release): sigiro ${tag}`,
      "--body",
      body,
    ]).trim();
    console.log(url);
  }
} finally {
  execFileSync("git", [
    "switch",
    "--quiet",
    "--force",
    ...(startRef === start ? [] : ["--detach"]),
    startRef,
  ]);
  if (dryRun && branch)
    execFileSync("git", ["branch", "--quiet", "-D", branch]);
}
