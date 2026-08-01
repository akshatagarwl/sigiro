import type { Metadata } from 'next';
import { SITE } from '@/lib/site';

/** A darkroom, not a page. It exists to be photographed, never to be found. */
export const metadata: Metadata = {
  robots: { index: false, follow: false },
};

/**
 * The Open Graph card, rendered as a real page so it can be screenshotted at
 * 1200×630 into `public/og.png`.
 *
 * Why a route and not `next/og`: Satori needs font buffers, and Geist arrives
 * here through `next/font`, which does not hand them out. Rendering the card as
 * a page instead means it inherits the real Geist, the real ink ramp and the
 * real materials from globals.css — the card cannot drift from the design
 * language because it *is* the design language.
 *
 * Regenerate after any change to `SITE.tagline`:
 *
 *   1. bun run dev
 *   2. screenshot http://localhost:3000/og at exactly 1200×630
 *   3. save it to public/og.png
 *
 * The card this replaced was a hand-made PNG, and both of its faults came from
 * being hand-made: the headline overflowed the canvas ("self-improving softwar"
 * with the last letters cut off, and "sigiro." clipped at the right edge), and
 * it still said "self-improving software and agents" months after the site
 * stopped. Reading the tagline from SITE is what stops the second one recurring.
 */
export default function OgCard() {
  return (
    <div
      className="flex flex-col justify-between bg-bezel"
      style={{ width: 1200, height: 630, padding: 72 }}
    >
      <div className="flex items-center gap-2.5">
        {/* The brand mark from §6: a cell, not a dot. There are no pills in this
            palette, and 5px at a 1.5px radius is the language's own unit. */}
        <span aria-hidden className="rounded-[1.5px] bg-accent" style={{ width: 14, height: 14 }} />
        <span className="font-medium text-ink" style={{ fontSize: 30, letterSpacing: '-0.03em' }}>
          sigiro
        </span>
      </div>

      {/* Medium, never bold (§8) — the negative tracking does the work that
          weight would do badly. The old card was bold, and at this size bold
          Geist is what made it look like a slide rather than a product. */}
      <h1
        className="font-medium text-balance text-ink"
        style={{ fontSize: 68, lineHeight: 1.08, letterSpacing: '-0.04em', maxWidth: 940 }}
      >
        {SITE.tagline}
      </h1>

      <p className="font-mono text-ink-3" style={{ fontSize: 22, letterSpacing: '0.01em' }}>
        sigiro.com · free when you install it
      </p>
    </div>
  );
}
