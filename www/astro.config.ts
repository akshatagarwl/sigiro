import cloudflare from '@astrojs/cloudflare';
import react from '@astrojs/react';
import sitemap from '@astrojs/sitemap';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig, fontProviders, sessionDrivers } from 'astro/config';
import { SITE } from './src/lib/site';

/**
 * The landing page, on Astro.
 *
 * It was Next on Vite via vinext until 2026-08-03. Nothing about the page needed
 * a React framework: it is one document, statically rendered, with four things on
 * it that run in the browser — the theme toggle, two scroll-driven terminals, an
 * accordion and a marquee. Next made every one of those a "client component"
 * inside an app that was otherwise React-rendering static markup at request time.
 * Astro renders the markup at build time and ships JavaScript only for the four
 * islands, which is the same architecture stated honestly.
 *
 * No `output: 'server'`. Every route here is prerendered except `/dl/[...key]`,
 * which opts out on its own and is the only reason an adapter is present at all.
 */
export default defineConfig({
  site: SITE.url,
  adapter: cloudflare({
    // The site has no <Image>. Naming the compile-time service stops the adapter
    // from wiring up a runtime one we would never call.
    imageService: 'compile',
  }),

  /**
   * Nothing on this site has a session. Saying so is not optional, because
   * `@astrojs/cloudflare` reads an unset `session.driver` as "wire up Cloudflare
   * KV" and injects a `SESSION` KV binding with no namespace id — which
   * `wrangler deploy` then silently provisions. The adapter has no switch for
   * "no sessions"; naming any driver is what turns it off, and `null` is the one
   * that stores nothing.
   *
   * `lruCache` and not `null`: unstorage ships a `null` driver and it is the honest
   * name for this, but Astro's `sessionDrivers` type does not list it (nor
   * `memory`), so it only compiles by lying to TypeScript. `lruCache` is in the
   * typed set, holds its data in the isolate, and provisions nothing. If sessions
   * are ever actually wanted here, this line is what to change — an in-memory store
   * in a Worker is per-isolate and will lose data.
   *
   * `wrangler deploy --dry-run` is where to check this: it lists the bindings the
   * Worker will get, and `env.SESSION` must not be among them.
   */
  session: { driver: sessionDrivers.lruCache() },

  integrations: [
    /**
     * Four islands, and nothing else crosses to the browser: `terminal.tsx`,
     * `faq.tsx`, `integrations.tsx` and `site/theme-toggle.tsx`. Every other
     * React component in this tree is rendered by Astro at build time and ships
     * no JavaScript — including the ones vendored from interior.dev that are
     * marked `'use client'`. That directive is a Next convention; Astro decides
     * hydration from the `client:*` attribute at the call site instead, which is
     * why the boundary is now visible in the page rather than in a file header.
     */
    react(),
    /** One page, so one URL — but generated rather than hand-maintained. */
    sitemap(),
  ],

  /**
   * Geist and Geist Mono, self-hosted from the `geist` package already in the
   * tree. Astro subsets them, emits the `@font-face` rules, preloads what the
   * page uses and generates fallback metrics so the swap does not shift layout.
   *
   * This replaced `next/font/google`, which self-hosted the same two families
   * but had a trap in it: setting `preload: false` silently switched it to
   * loading the woff2 from fonts.gstatic.com, sending every visitor's IP to
   * Google. There is no such mode here — the files are local, always.
   *
   * The paths reach into `node_modules` because `geist` exports only its
   * `./font*` subpaths, so the woff2 files are not importable by specifier. A
   * missing file fails the build, which is the failure mode we want.
   */
  fonts: [
    {
      provider: fontProviders.local(),
      name: 'Geist',
      cssVariable: '--font-geist-sans',
      options: {
        variants: [
          {
            weight: '100 900',
            style: 'normal',
            src: ['./node_modules/geist/dist/fonts/geist-sans/Geist-Variable.woff2'],
          },
        ],
      },
    },
    {
      provider: fontProviders.local(),
      name: 'Geist Mono',
      cssVariable: '--font-geist-mono',
      options: {
        variants: [
          {
            weight: '100 900',
            style: 'normal',
            src: ['./node_modules/geist/dist/fonts/geist-mono/GeistMono-Variable.woff2'],
          },
        ],
      },
    },
  ],

  vite: { plugins: [tailwindcss()] },
});
