#!/usr/bin/env bun
/**
 * Sync docs/ -> www/content/docs/
 *
 * One source of truth: docs/ contains all content as .md files.
 * This script copies them into www/content/docs/ as .md files
 * so Fumadocs can build them with format: 'md' (plain markdown).
 *
 * Rules:
 *   - Skip dirs: .obsidian, self-loop
 *   - Skip files without a title field in frontmatter
 *   - Auto-quote title values that contain unquoted colons
 */
import { readdir, mkdir, readFile, writeFile, rm } from "fs/promises";
import path from "path";

const SRC = path.resolve(import.meta.dirname, "../../docs");
const DST = path.resolve(import.meta.dirname, "../content/docs");

const EXCLUDE_DIRS = new Set([".obsidian", "self-loop"]);

function hasTitle(frontmatter: string): boolean {
  return /^title:/m.test(frontmatter);
}

function fixTitle(frontmatter: string): string {
  // Quote titles that contain colons but aren't already quoted
  return frontmatter.replace(
    /^title:\s*(?!"|'|`)(.+)$/gm,
    (match, value) => {
      if (value.includes(":")) {
        return `title: "${value.replace(/"/g, '\\"')}"`;
      }
      return match;
    }
  );
}

async function syncDir(srcDir: string, dstDir: string) {
  await mkdir(dstDir, { recursive: true });
  const entries = await readdir(srcDir, { withFileTypes: true });

  for (const entry of entries) {
    const srcPath = path.join(srcDir, entry.name);
    const dstPath = path.join(dstDir, entry.name);

    if (entry.isDirectory()) {
      if (EXCLUDE_DIRS.has(entry.name)) continue;
      await syncDir(srcPath, dstPath);
      continue;
    }

    if (!entry.name.endsWith(".md")) continue;

    let content = await readFile(srcPath, "utf-8");

    // Must have frontmatter with a title
    const fmMatch = content.match(/^---\n([\s\S]*?)\n---/);
    if (!fmMatch || !hasTitle(fmMatch[1])) {
      if (entry.name === "index.md") {
        content =
          '---\ntitle: Sigiro Documentation\ndescription: Self-operating software factory\n---\n\n' +
          content;
      } else {
        continue;
      }
    }

    // Fix unquoted colons in titles
    content = content.replace(
      /^---\n([\s\S]*?)\n---/,
      (match, fm) => `---\n${fixTitle(fm)}\n---`
    );

    await writeFile(dstPath, content, "utf-8");
  }
}

await rm(DST, { recursive: true, force: true });
await syncDir(SRC, DST);

console.log(`[sync-docs] ${SRC} -> ${DST}`);
