# www

sigiro.com — one page, built with [Astro](https://astro.build) and deployed to
Cloudflare Workers.

This file was the untouched `create-next-app` boilerplate until 2026-08-03, which
is how long it had been out of date: the site has since been a Vite SPA, a Next.js
app on Vite via vinext, and now Astro.

## Commands

```bash
bun install
bun run dev           # astro dev
bun run build         # astro build → dist/client (assets) + dist/server (the Worker)
bun run preview       # wrangler dev on the built Worker — the real runtime
bun run deploy        # wrangler deploy
bun run lint          # oxlint
bun run fmt           # oxfmt
bun run types:check   # astro sync && tsc --noEmit
bun run og            # regenerate the Open Graph card into public/
```

Always verify with `bun run preview` rather than `astro dev`. The two run in
different engines — `astro dev` is Node, the deployed site is workerd — and at
least one bug has only ever appeared in the second one (see the Shiki note in
`src/lib/highlight.ts`).

## What is where

| Path | What it is |
| --- | --- |
| `src/pages/index.astro` | the page — every section, in order |
| `src/layouts/Base.astro` | the document, the panel, the header and the footer |
| `src/lib/site.ts` | every word the site says. Copy lives here, not in markup |
| `src/lib/highlight.ts` | Shiki, configured for the Workers runtime |
| `src/components/interior/` | vendored from interior.dev, unmodified — see DESIGN.md §1b |
| `src/pages/dl/[...key].ts` | release downloads out of R2. The one route that is not prerendered |
| `public/install` | the script `curl -fsSL sigiro.com/install \| sh` fetches |
| `public/_headers`, `public/_redirects` | caching, and the published short links |
| `DESIGN.md` | the design language. Read it before changing anything visual |

Copy and positioning are governed by `.agents/product-marketing.md` at the repo
root, and all user-facing text follows ASD-STE100 Simplified Technical English.

## Four islands

Everything renders to HTML at build time. Four components ship JavaScript, and each
says so at the point it is used:

- `site/theme-toggle.tsx` — `client:load`
- `terminal.tsx` — `client:visible`, twice
- `faq.tsx` — `client:visible`
- `integrations.tsx` — `client:visible`

Nothing else does. A React component rendered from `.astro` with no `client:*`
directive is static markup, including the ones still marked `'use client'` — that
directive is a Next convention and Astro ignores it.

## The docs are not here

They are [Blume](https://useblume.dev) at docs.sigiro.com, in `docs-site/`.
`/docs/*` on this origin redirects there; the rules are in `public/_redirects`.
