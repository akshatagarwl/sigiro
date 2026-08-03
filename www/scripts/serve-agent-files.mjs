/**
 * Move Blume's agent-facing build artifacts into the directory the Worker serves.
 *
 * This is a bridge over an upstream defect, and it is here rather than in an
 * integration because there is no native seam to use — I looked for all three:
 *
 * - Blume's `deployStaticDir()` returns `dist/client` for `adapter: 'node'` and
 *   plain `dist` for `cloudflare`. But `@astrojs/cloudflare` in server mode serves
 *   `dist/client` (its generated wrangler config sets `assets.directory:
 *   "../client"`), so on Cloudflare these land one directory above anything the
 *   Worker can reach. The Cloudflare case belongs in the same branch as node —
 *   a one-line upstream fix.
 * - There is no Blume config knob: `deployment` accepts only `adapter`, `base`,
 *   `output` and `site`, and `distDir` is fixed at `<root>/dist` unless
 *   `--isolated`.
 * - There is no Blume build hook (`blume/hooks` is island React hooks), and Blume
 *   writes these files *after* `astro build` returns, so no Astro integration hook
 *   fires late enough to catch them.
 *
 * `sitemap.xml` and `robots.txt` are deliberately NOT in this list. Those two do
 * have native substitutes — `@astrojs/sitemap` and a `public/robots.txt` — which
 * write into Astro's own client output and are the ones actually served. Moving
 * Blume's copies over them would clobber ours with the unreachable versions.
 *
 * Still present in Blume 1.3.1, re-checked on upgrade from 1.2.1: every file in
 * FILES below had to be moved, so nothing here is dead yet. `robots.txt` and
 * `sitemap.xml` are still written to the unreachable level too.
 *
 * When the upstream fix lands, Blume will write these straight into `dist/client`
 * and this script becomes a no-op that says so — then delete it.
 */
import { existsSync } from 'node:fs';
import { mkdir, rename, stat } from 'node:fs/promises';
import { dirname, join } from 'node:path';

/**
 * The artifacts with no Astro-native generator to substitute.
 *
 * Paths, not names: `.well-known/api-catalog` is nested, so a target directory
 * may not exist yet. It does today only because the MCP endpoint happens to write
 * `.well-known/mcp.json` beside it — turning MCP off would leave nothing to
 * create it, which is the kind of coupling that fails a build months later.
 *
 * `api-catalog` (RFC 9727) arrived with Blume 1.3.0 and is not optional to move:
 * `agent-readability.json` — which this script puts where it IS served —
 * advertises `apiCatalog: https://sigiro.com/.well-known/api-catalog`. Left
 * behind, the one file agents read to find everything else points at a 404.
 */
const FILES = ['llms.txt', 'llms-full.txt', 'agent-readability.json', '.well-known/api-catalog'];

const dist = join(process.cwd(), 'dist');
const client = join(dist, 'client');

if (!existsSync(client)) {
  // Not a warning. A missing client dir means the build shape changed under us,
  // and guessing is how a silent 404 ships.
  throw new Error(
    `serve-agent-files: ${client} does not exist. Run \`blume build\` first, or the ` +
      `adapter's output layout has changed and this script needs revisiting.`,
  );
}

const moved = [];
const already = [];
const missing = [];

for (const name of FILES) {
  const from = join(dist, name);
  const to = join(client, name);

  if (existsSync(to)) {
    // Upstream fixed it, or a previous run already moved this one.
    already.push(name);
    continue;
  }
  if (!existsSync(from)) {
    missing.push(name);
    continue;
  }
  await mkdir(dirname(to), { recursive: true });
  await rename(from, to);
  const { size } = await stat(to);
  moved.push(`${name} (${size} bytes)`);
}

if (missing.length) {
  /**
   * Loud, and fatal. These paths are published — `/llms.txt` is a convention
   * crawlers and agents look for at the site root — so a build that quietly
   * produced none of them would ship a 404 that nobody notices until an agent
   * fails to read the docs. If a file is intentionally disabled, take it out of
   * FILES above rather than letting this pass.
   */
  throw new Error(
    `serve-agent-files: Blume generated none of [${missing.join(', ')}] in ${dist}. ` +
      `Either the generators are disabled in blume.config.ts (then remove them from ` +
      `FILES) or their output location changed.`,
  );
}

if (moved.length) console.log(`serve-agent-files: moved ${moved.join(', ')} into dist/client`);
if (already.length) {
  console.log(
    `serve-agent-files: ${already.join(', ')} already in dist/client — if that is all of ` +
      `them, the upstream fix has landed and this script can be deleted`,
  );
}
