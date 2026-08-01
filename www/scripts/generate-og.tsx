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
 * The renderer is `next/og`, which wraps `@vercel/og` and already bundles satori,
 * `resvg.wasm` and `yoga.wasm`. The fonts come from `geist`, Vercel's own
 * package, which ships the family as TTF — the format satori reads.
 *
 * That font package is the whole reason this card is on-spec. An earlier version
 * used the single `Geist-Regular.ttf` that @vercel/og bundles and set the
 * headline at weight 400, with a comment explaining that §8 wanted medium and
 * satori would not synthesise a weight. The premise was wrong: satori did not
 * need to synthesise anything, the medium file simply was not in the tree. It is
 * a missing asset, not a missing capability — worth remembering the next time a
 * renderer looks like it cannot do something.
 */

import { readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
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

/**
 * Resolved through the package rather than by a hand-written node_modules path,
 * so a hoist or a version bump cannot silently point this at nothing.
 */
const FONTS = join(dirname(require.resolve('geist/package.json')), 'dist', 'fonts');

const font = (family: 'geist-sans' | 'geist-mono', file: string) =>
  readFile(join(FONTS, family, file));

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
      <div style={{ fontSize: 30, fontWeight: 500, letterSpacing: '-0.03em' }}>sigiro</div>
    </div>

    {/* -0.04em, per §8, and it renders cleanly.

        It did not always. With the regular weight this same tracking opened a
        gap between "software" and "that" wide enough to look like a double space
        in the source — it was not, the string is single-spaced ASCII, checked. I
        blamed satori's tracking implementation and dropped the tracking. Wrong
        diagnosis: the medium file shapes it correctly, so the fault was the font,
        the same font that was missing in the first place.

        Satori does implement no `text-balance`, though, so the measure does that
        job: 880 gives two lines, where 940 orphaned "repairs itself." and 760
        pushed it to three. */}
    <div
      style={{
        fontSize: 68,
        fontWeight: 500,
        lineHeight: 1.08,
        letterSpacing: '-0.04em',
        maxWidth: 880,
      }}
    >
      {SITE.tagline}
    </div>

    {/* Mono, matching every `.meta` line on the site: metadata and numbers, never
        prose (§8). It was sans while only the sans regular was in the tree. */}
    <div style={{ fontFamily: 'Geist Mono', fontSize: 22, color: INK_3 }}>
      sigiro.com · free when you install it
    </div>
  </div>
);

const png = Buffer.from(
  await new ImageResponse(card, {
    width: WIDTH,
    height: HEIGHT,
    fonts: [
      { name: 'Geist', data: await font('geist-sans', 'Geist-Medium.ttf'), weight: 500 },
      { name: 'Geist Mono', data: await font('geist-mono', 'GeistMono-Regular.ttf'), weight: 400 },
    ],
  }).arrayBuffer(),
);

// `SITE.ogImage` is the path the meta tags publish, so writing to it is what
// keeps the asset and the tag from drifting apart.
const out = join(import.meta.dirname, '..', 'public', SITE.ogImage.replace(/^\//, ''));
await writeFile(out, png);

console.log(`og: ${out} (${WIDTH}x${HEIGHT}, ${Math.round(png.byteLength / 1024)} KB)`);
