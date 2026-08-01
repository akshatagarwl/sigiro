/**
 * Generates the Open Graph card into `public/`.
 *
 *   bun run og
 *
 * Run it after any change to `SITE.tagline`, then bump `SITE.ogImage` to a new
 * filename — scrapers cache by URL and WhatsApp's cache cannot be purged, so a
 * new path is the only reliable invalidation.
 *
 * This replaced a route that was screenshotted by hand. The route worked and was
 * the wrong shape: a card is an asset with one input, not a page, and "open a
 * browser and photograph it" is not a build step anyone will repeat correctly.
 *
 * Everything needed is already in the tree — `next/og` wraps `@vercel/og`, which
 * bundles satori, `resvg.wasm`, `yoga.wasm` and, usefully, `Geist-Regular.ttf`.
 * No new dependency.
 *
 * One deviation from the design language, on purpose: §8 sets headings in medium
 * and never bold, but only the regular weight ships with @vercel/og. Satori will
 * not synthesise a weight, and pulling a whole font package for one step is a
 * poor trade at 68px, where the -0.04em tracking is doing most of the work.
 * Registering the regular file under `weight: 500` would render the same pixels
 * while claiming otherwise, so the weight is declared honestly as 400.
 */

import { readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { ImageResponse } from 'next/og';
import { SITE } from '../src/lib/site';

/** The card, per the OG spec and what every scraper crops to. */
const WIDTH = 1200;
const HEIGHT = 630;

/** From globals.css. Literals, because satori resolves no CSS custom property. */
const BEZEL = '#EFEEEA';
const INK = '#1b1b19';
const INK_3 = '#6e6e66';
const ACCENT = '#4568ff';

const require = createRequire(import.meta.url);

async function geist() {
  // Resolved through `next`, so it follows the installed version rather than a
  // path that quietly rots the next time next is upgraded.
  const og = require.resolve('next/dist/compiled/@vercel/og/index.node.js');
  return readFile(join(og, '..', 'Geist-Regular.ttf'));
}

const card = (
  <div
    style={{
      width: WIDTH,
      height: HEIGHT,
      padding: 72,
      background: BEZEL,
      color: INK,
      display: 'flex',
      flexDirection: 'column',
      justifyContent: 'space-between',
      fontFamily: 'Geist',
    }}
  >
    <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
      {/* §6: a cell, not a dot. There are no pills in this palette. */}
      <div style={{ width: 14, height: 14, borderRadius: 1.5, background: ACCENT }} />
      <div style={{ fontSize: 30, letterSpacing: '-0.03em' }}>sigiro</div>
    </div>

    {/* No `letterSpacing` here, though §8 asks for -0.04em on a headline and the
        wordmark above keeps its -0.03em happily.

        At this size satori's tracking implementation distributes the negative
        space unevenly across word boundaries: the render came out with a visible
        gap between "software" and "that", wide enough to read as a double space
        in the source. It is not — the string is single-spaced ASCII, checked.
        Dropping the tracking removed the gap and, as a side effect, let the line
        break where the sentence's sense breaks.

        Satori also implements no `text-balance`, so the measure does that job
        instead: 880 gives two lines, where 940 orphaned "repairs itself." and 760
        pushed it to three. */}
    <div style={{ fontSize: 68, lineHeight: 1.08, maxWidth: 880 }}>{SITE.tagline}</div>

    <div style={{ fontSize: 22, color: INK_3 }}>sigiro.com · free when you install it</div>
  </div>
);

const png = Buffer.from(
  await new ImageResponse(card, {
    width: WIDTH,
    height: HEIGHT,
    fonts: [{ name: 'Geist', data: await geist(), weight: 400, style: 'normal' }],
  }).arrayBuffer(),
);

// `SITE.ogImage` is the path the meta tags publish, so writing to it is what
// keeps the asset and the tag from drifting apart.
const out = join(import.meta.dirname, '..', 'public', SITE.ogImage.replace(/^\//, ''));
await writeFile(out, png);

console.log(`og: ${out} (${WIDTH}x${HEIGHT}, ${Math.round(png.byteLength / 1024)} KB)`);
