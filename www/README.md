# www

sigiro.com — one page, built with [Astro](https://astro.build) and deployed to
Cloudflare Workers.

This file was the untouched `create-next-app` boilerplate until 2026-08-03, which
is how long it had been out of date: the site has since been a Vite SPA, a Next.js
app on Vite via vinext, and now Astro.

## Commands

```bash
aube install --frozen-lockfile
aube run dev           # astro dev
aube run build         # astro build → dist/client (assets) + dist/server (the Worker)
aube run preview       # local workerd preview of the wrapper Worker
aube run check:edge    # sub-second auth/API routing smoke test against the current build
aube run check:edge:full # rebuild, then run the routing smoke test
aube run deploy        # Alchemy deploys the wrapper Worker and infrastructure
aube run lint          # oxlint
aube run fmt           # Vite+ formatter using vite.config.ts
aube run check         # formatting, linting, and type checks
aube run og            # regenerate the Open Graph card into public/
```

`aube run` installs missing or stale dependencies before starting the script and
keeps the committed `bun.lock`; do not add a second lockfile. `aube run dev` is
for page development only. Use `aube run check:edge` while editing the wrapper;
it starts isolated mock origins and checks the complete route split, request
forwarding, redirect rewriting, and canonical-host redirect. Use
`check:edge:full` after changing Blume or static assets. The sole production
deployment path is Alchemy, which deploys `src/worker.ts`; do not deploy the
generated `dist/server/wrangler.json` directly because it contains only Blume's
inner Worker and bypasses the apex auth and API proxies.

## Alchemy migration

Two infrastructure composition roots share Alchemy's remote Cloudflare state backend:

- `alchemy.run.ts` declares the production zone, downloads bucket, website Worker, binding, routes, and Email Sending domain.
- `alchemy.hetzner.run.ts` adopts and retains the production Hetzner server, primary IPs, firewall, and SSH key.

Review both plans with:

```bash
aube run alchemy:plan
HCLOUD_TOKEN="$HCLOUD_TOKEN" aube run alchemy:hetzner:plan
```

Both scripts fix Alchemy's stage to production. Planning does not apply, deploy, or destroy resources. The Hetzner plan must be fully no-op after adoption.

## What is where

| Path                                   | What it is                                                                                                                                                                                                                 |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/pages/index.astro`                | the page — every section, in order                                                                                                                                                                                         |
| `src/layouts/Base.astro`               | the document, the panel, the header and the footer                                                                                                                                                                         |
| `src/lib/site.ts`                      | every word the site says. Copy lives here, not in markup                                                                                                                                                                   |
| `src/lib/highlight.ts`                 | Shiki, configured for the Workers runtime                                                                                                                                                                                  |
| `src/components/interior/`             | vendored from interior.dev — see DESIGN.md §1b. `task-steps.tsx` is no longer verbatim: its `TaskSteps` view was replaced by `stage-list.tsx` and the dead half was removed, so re-syncing that one file is a manual merge |
| `src/pages/dl/[...key].ts`             | release downloads out of R2. The one route that is not prerendered                                                                                                                                                         |
| `public/install`                       | the script `curl -fsSL sigiro.com/install \| sh` fetches                                                                                                                                                                   |
| `public/_headers`, `public/_redirects` | caching, and the published short links                                                                                                                                                                                     |
| `DESIGN.md`                            | the design language. Read it before changing anything visual                                                                                                                                                               |

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

## Public routing

The landing page and [Blume](https://useblume.dev) documentation are served from
`sigiro.com`; documentation lives under `/docs`. The Alchemy-deployed wrapper
Worker routes only the public auth, API, protected-resource discovery, and
OTLP/HTTP paths to their origin-only services. `www.sigiro.com` redirects to the
single canonical origin before any authentication flow runs.
