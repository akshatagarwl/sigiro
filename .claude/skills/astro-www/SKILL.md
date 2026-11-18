---
name: astro-www
description: Build and deploy the sigiro website (www/) — an Astro site with React islands, deployed to Cloudflare Workers. Load when touching www/astro.config.ts, www/wrangler.jsonc, www/package.json scripts, running dev/build/preview, deploying the site, or deciding whether a component should hydrate.
---

# astro-www (www/)

One page, Astro, React islands, Cloudflare Workers. This replaced a Next.js App
Router app running on Vite via vinext on 2026-08-03; the skill that described that
setup was this file, and everything it said is now wrong. What survived is at the
bottom, under "Things that cost hours".

Vite+ stays for `vp lint`. `oxfmt` formats; it does not touch `.astro`.

## Shape

```
src/pages/index.astro        the page
src/layouts/Base.astro       document + panel + header + footer
src/pages/dl/[...key].ts     R2 downloads — the ONE route with prerender = false
src/lib/site.ts              all copy
public/_headers _redirects   caching and the short links
```

`output` is left at its default (`static`). The adapter exists for that one
endpoint. Do not switch to `output: 'server'` to make something work — check
whether the thing actually needs a request first, because almost nothing here does.

## Commands

```bash
cd www
aube run dev      # astro dev — Node engine, NOT the deploy target
aube run build    # astro build
aube run preview  # wrangler dev on the built Worker — the real runtime
aube run deploy   # wrangler deploy --config dist/server/wrangler.json
aube run check    # formatting, linting, and type checks
```

## Hydration is a call-site decision

A React component rendered from `.astro` with no `client:*` directive is static
markup and ships zero JavaScript. `'use client'` means nothing to Astro — several
components in `components/interior/` still carry it from the Next era and render
statically anyway.

Four islands, and that number should not grow without a reason:
`site/theme-toggle.tsx` (`client:load`), `terminal.tsx` (`client:visible`, twice),
`faq.tsx`, `integrations.tsx`.

Where a component needs both build-time work and browser behaviour, the build-time
half goes in a `.astro` file and the browser half is the island it mounts.
`FactoryLoop.astro` and `HowItWorks.astro` are that split: they run Shiki in
frontmatter and hand the highlighted markup to `SteppedTerminal` as props.

Island props are serialised to JSON in the HTML, so a large prop is paid for twice
— once in the SSR'd markup, once in the props. The no-JS fallbacks live in a
`<noscript>` in the `.astro` file for exactly this reason; as a `fallback` prop
they were shipped to every reader who never sees them.

## Config gotchas found the hard way

- **`wrangler.jsonc` must not set `main` or `assets`.** The adapter writes both
  into `dist/server/wrangler.json`, and that generated file is what `deploy` ships.
  A stale `main` here does not get overridden — it fails the build in the
  Cloudflare Vite plugin with "Failed to resolve main entry file".
- **`session` must name a driver.** `@astrojs/cloudflare` reads an unset
  `session.driver` as "use Cloudflare KV" and injects a `SESSION` KV binding with
  no namespace id, which `wrangler deploy` then provisions. Nothing here has a
  session. `sessionDrivers.null()` turns it off; there is no `false`. Check with
  `wrangler deploy --dry-run`, which lists the bindings.
- **`_headers` must not use `/*`.** The adapter appends the immutable
  `Cache-Control` for `/_astro/*` only when nothing already matches that path, and
  a `/*` rule matches it — so a catch-all silently downgrades every hashed asset
  from a year to whatever the catch-all says. Name the document.
- **`Font` is imported from `astro:assets`, not `astro:fonts`.** The latter is the
  config section's name and is not a module.
- **`Astro.locals.runtime.env` is gone** (removed in Astro v6). Bindings come from
  `import { env } from 'cloudflare:workers'`. The error message says so, which is
  the good case.
- **`astro check` does not run in this repo.** It needs TypeScript's 6.x
  programmatic API and this repo is on the 7.x native compiler. `types:check` is
  `astro sync && tsc --noEmit` instead, which covers every `.ts`/`.tsx` but **not**
  the `.astro` templates — those are only checked by `astro build`. If a type bug
  in a `.astro` file ever ships, that gap is why.

## Things that cost hours here — don't rediscover them

These predate Astro and still hold.

- **Shiki must not use the WASM engine.** `codeToHtml` from `shiki` loads
  oniguruma, which renders fine under Node and **500s on Workers**. See
  `src/lib/highlight.ts` — `createHighlighterCore` + `createJavaScriptRegexEngine`
  + fine-grained lang/theme imports, built once per isolate at module scope. This is
  the reason to verify with `preview` and not `dev`.
- **Never trust a Lighthouse run against a cold deploy.** A single run swung
  between 76 and 98 on identical builds. Warm the edge with a few requests, then
  take the median of three — a "regression" measured cold sent me reverting a change
  that was worth 14 points.
- Right after `deploy`, the first request or two can serve mid-propagation state.
  Wait for every asset referenced by `/` to return 200 before measuring anything.
- **Fonts must stay self-hosted.** Under `next/font`, `preload: false` silently
  switched to loading the woff2 from `fonts.gstatic.com` — every visitor's IP to
  Google, dressed as a performance win because the third party dropped off the
  measured critical path. Astro's Fonts API has no such mode: the files are read
  from `node_modules/geist`. Keep it that way, and do not "simplify" it to a Google
  provider.
- **A theme toggle needs a pre-paint inline script.** It cannot be a bundled script
  and it cannot live in the island — both run after paint, which is the flash the
  script exists to prevent. `Base.astro` has it, `components/site/theme.ts` has the
  runtime half, and the three lines they share are duplicated on purpose.
- **The server snapshot of `useSyncExternalStore` is load-bearing.** Both the theme
  toggle and the install picker read browser state, and both return a constant third
  value ("not known yet") from `getServerSnapshot`. React feeds that same snapshot
  to the hydrating render, which is what makes the markup agree. Reading storage
  during render instead is what made React throw the hydrated tree away under
  `next-themes`.

## Where the docs are

Not here. They are Blume in `docs-site/`, deployed to docs.sigiro.com. `/docs/*`
redirects there from `public/_redirects`. Anything about MDX, fumadocs,
`fumadocs-openapi`, react-markdown or shadcn registry components in this repo's
history is dead — those dependencies are gone, and so are `www/components.json`
and `www/.source/`.
