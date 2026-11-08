# www

sigiro.com — one page, built with [Astro](https://astro.build) and deployed to
Cloudflare Workers.

This file was the untouched `create-next-app` boilerplate until 2026-08-03, which
is how long it had been out of date: the site has since been a Vite SPA, a Next.js
app on Vite via vinext, and now Astro.

## Commands

```bash
# Run from the repository root
aube install --frozen-lockfile
aube run fmt
aube run fmt:check
aube run lint

aube -C www install --frozen-lockfile
aube -C www run dev       # astro dev
aube -C www run build     # astro build → dist/client (assets) + dist/server (the Worker)
aube -C www run preview  # local workerd preview of the wrapper Worker
aube -C www run deploy   # Alchemy deploys the wrapper Worker and infrastructure
aube -C www run check    # repository formatting and linting
aube -C www run og       # regenerate the Open Graph card into public/
```

The repository-root `oxfmt.config.ts` and `oxlint.config.ts` are shared by auth,
the website, and the agent. `aube run` installs missing or stale dependencies
before starting a package script and uses that package's committed `aube-lock.yaml`;
do not add a second lockfile. `aube -C www run dev` is for page development only. Validate routing by driving the running Worker with a
real agent and inspecting traces and call-stack diffs. The sole production
deployment path is Alchemy, which deploys `src/worker.ts`; do not deploy the
generated `dist/server/wrangler.json` directly because it contains only Blume's
inner Worker and bypasses the apex auth and API proxies.

## Alchemy migration

`alchemy.run.ts` declares the production zone, downloads bucket, website Worker, binding, routes, and Email Sending domain, with state in Alchemy's remote Cloudflare state backend. Review its plan with:

```bash
mise run alchemy-plan
```

The production host stacks (server adoption and the native workload) live in a separate private infrastructure repository.

Both scripts fix Alchemy's stage to production. Planning does not apply, deploy, or destroy resources. The Hetzner plan must be fully no-op after adoption.

## What is where

| Path                                   | What it is                                                                                                                                                                                                                 |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/pages/index.astro`                | the page — every section, in order                                                                                                                                                                                         |
| `src/layouts/Base.astro`               | the document, the panel, the header and the footer                                                                                                                                                                         |
| `src/lib/site.ts`                      | every word the site says. Copy lives here, not in markup                                                                                                                                                                   |
| `src/lib/highlight.ts`                 | Shiki, configured for the Workers runtime                                                                                                                                                                                  |
| `src/components/ui/` | stock shadcn/ui components, copied from the registry. Keep them unedited; the theme is `theme.css` |
| `src/pages/dl/[...key].ts`             | release downloads out of R2. The one route that is not prerendered                                                                                                                                                         |
| `public/install`                       | the script `curl -fsSL sigiro.com/install \| sh` fetches                                                                                                                                                                   |
| `public/_headers`, `public/_redirects` | caching, and the published short links                                                                                                                                                                                     |

All user-facing text follows ASD-STE100 Simplified Technical English.

## Four islands

Everything renders to HTML at build time. Four components ship JavaScript, and each
says so at the point it is used:

- `site/theme-toggle.tsx` — `client:load`
- `terminal.tsx` — `client:visible`, twice
- `faq.tsx` — `client:visible`
- `integrations.tsx` — `client:visible`

`/dashboard/` is not part of this site: it is its own app in `../dashboard`
(a Next static export on the `Dashboard` Worker, routed at `sigiro.com/dashboard*`
in `alchemy.run.ts`). `aube run check:dashboard` drives its build.

Nothing else on the landing page does. A React component rendered from `.astro` with no `client:*`
directive is static markup, including the ones still marked `'use client'` — that
directive is a Next convention and Astro ignores it.

## Public routing

The landing page and [Blume](https://useblume.dev) documentation are served from
`sigiro.com`; documentation lives under `/docs`. The Alchemy-deployed wrapper
Worker routes only the public auth, API, protected-resource discovery, and
OTLP/HTTP paths to their origin-only services. `www.sigiro.com` redirects to the
single canonical origin before any authentication flow runs.
