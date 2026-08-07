import { fileURLToPath } from 'node:url';
import sitemap from '@astrojs/sitemap';
import { defineConfig } from 'blume';
import type { AstroIntegration } from 'astro';

/**
 * The two things this project needs from Astro that Blume's config cannot express.
 *
 * `integrations` is Blume's documented passthrough, and both of these are what an
 * integration is for. Blume re-imports this config from the generated Astro config,
 * so the module evaluates twice per run — this factory returns an object and
 * touches nothing else, which is the requirement it states.
 *
 * **1. `@/…` → `./src`.** Blume builds its Astro app in `.blume/`, so that is where
 * Astro reads `tsconfig.json` from and a `paths` alias in ours is never seen. The
 * landing page and its twenty-odd components came from `www/` importing
 * `@/components/…` and `@/lib/…`; rewriting every one to a relative path would be a
 * large diff that makes the tree harder to read, not easier.
 *
 * **2. `/dl/[...key]`.** Release artifacts streamed out of R2, and the only route
 * on this site that is not prerendered. Blume's `pages/` folder mounts `.astro`
 * files only — a `pages/dl/[...key].ts` was copied in and silently never mounted,
 * which showed up as an Astro 404 with `DOWNLOADS` appearing in the generated
 * wrangler config and in no built code at all. `injectRoute` is the supported way
 * in, and the way Blume mounts its own `.well-known/mcp.json` endpoint.
 */
const sigiro = (): AstroIntegration => ({
  name: 'sigiro',
  hooks: {
    'astro:config:setup': ({ injectRoute, updateConfig }) => {
      updateConfig({
        vite: {
          resolve: {
            alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
          },
        },
      });
      injectRoute({
        pattern: '/dl/[...key]',
        entrypoint: fileURLToPath(new URL('./src/dl.ts', import.meta.url)),
      });
    },
  },
});

/**
 * sigiro.com — the landing page at `/` and the docs at `/docs/*`.
 *
 * This replaced a fumadocs install inside a Next app at sigiro.com/docs, went to
 * docs.sigiro.com for two days, and came back. The rule for this file is that
 * everything here is configuration: no custom MDX components, no bespoke API
 * reference. The parts that were custom before — a 177-line operation renderer, a
 * generator that turned our OpenAPI spec into MDX, two llms.txt route handlers, a
 * 182-line stylesheet mapping fumadocs' variables onto our tokens — are all
 * options below.
 *
 * The landing page is `pages/index.astro` with its own shell in
 * `src/layouts/Base.astro`, because it keeps its own header rather than the docs
 * one. Everything else about it is Blume's: the fonts, the Tailwind entry, the
 * theme attribute and its storage key.
 *
 * Being one origin is what lets the landing page and a docs page share a
 * `localStorage` preference, which the two-hostname split could not — storage is
 * per-origin, and that was the one thing genuinely lost while the docs lived on
 * their own subdomain.
 */
export default defineConfig({
  title: 'sigiro',
  description:
    'Send OpenTelemetry to sigiro. When a service fails, you get the cause and the query that proves it.',

  logo: { text: 'sigiro', href: '/' },

  /**
   * The docs sit under `/docs`, the landing page owns `/`.
   *
   * `basePath` prepends this to every generated route while staying invisible to
   * the sidebar and nav tree, which is exactly the shape wanted here — the docs
   * read as a section of one site rather than as a separate property. It is
   * distinct from `deployment.base` (Astro's host-subdirectory base), which is
   * not in play.
   *
   * This returns the docs to the URLs they had under fumadocs. They spent two
   * days at docs.sigiro.com; that Worker, its routes and its custom domain are
   * deleted, so the hostname no longer resolves at all.
   */
  basePath: '/docs',

  /**
   * `sigiro()` is the alias + `/dl` route, above.
   *
   * `@astrojs/sitemap` is here to route around a Blume defect, and it is the fix
   * Blume's own customization guide reaches for — `integrations` is documented
   * with `@astrojs/sitemap` as its example.
   *
   * The defect, re-confirmed on 1.3.1: Blume writes its build artifacts to
   * `deployStaticDir()`, and that function reasons "Netlify publishes `dist/`
   * itself and Cloudflare serves the `outDir` root, so every other build serves
   * `dist/`". True for a Cloudflare *static* build. False for a Cloudflare
   * *server* build, where `@astrojs/cloudflare` writes `assets.directory:
   * "../client"` and serves `dist/client` — so every artifact lands one directory
   * above anything the Worker can reach. The fix is one clause: `cloudflare`
   * belongs in the same `output === 'server'` branch as `node`.
   *
   * What is native rather than a workaround: an Astro *integration* writes into
   * Astro's own client output, so its file is served. `@astrojs/sitemap` is a real
   * sitemap generator that happens to put the file in the right place, and
   * `public/robots.txt` is a real static asset — and `seo.robots` is now off, so
   * Blume no longer writes a robots copy nobody could read.
   *
   * `seo.sitemap` stays on, and the note on it below says why that is not
   * inconsistent.
   *
   * NOT fixable this way: `llms.txt`, `llms-full.txt`, `agent-readability.json`
   * and `.well-known/api-catalog`. There is no Astro-native generator to
   * substitute — they are Blume's own content pipeline, `buildLlmsFiles` needs a
   * build-time `BlumeProject` an Astro route cannot reach, and Blume writes them
   * after `astro build` returns, so no integration hook fires late enough to
   * catch them. `scripts/serve-agent-files.mjs` moves those four and says so.
   */
  integrations: [sigiro(), sitemap()],

  /**
   * Retired URLs. Two groups, two different provenances — see the comment above
   * each one.
   *
   * These were `public/_redirects` rules. Blume expresses them itself, and on a
   * `server` build it handles them at request time through Astro's own
   * `redirects` — so they are config rather than a platform file, and there is one
   * fewer place to look for "what does this URL do".
   *
   * Written WITHOUT the `/docs` prefix on purpose. Blume bases both sides against
   * `basePath`, so `/api/get-trace` is matched at `/docs/api/get-trace` and lands
   * at `/docs/api`. Writing the prefix here would double it.
   *
   * The gain over `_redirects` is not tidiness: `blume audit`'s link checker reads
   * this list, so a page linking to a retired URL is a known redirect rather than
   * a broken link. A `_redirects` rule is invisible to it. That is also why the
   * Diátaxis group below belongs here rather than in `public/_redirects`: those
   * rules are all `/docs`-relative, so they are expressible natively, and the file
   * exists only for the root-level rules Blume cannot express.
   *
   * 302 throughout, never 301: the docs have moved origin twice in three days, and
   * a 301 is cached by the browser more or less forever. Every entry below is
   * ALSO listed as a negative `run_worker_first` rule in `wrangler.jsonc` — that
   * is what makes the 302 survive to the edge, and the note there says why.
   */
  redirects: [
    /* GROUP 1 — the API reference's old per-operation URLs.
     *
     * Closed set, and enumerated because it has to be: these came from two
     * generations of `fumadocs-openapi`, both deleted, so nothing can add a
     * twelfth *of this kind*. Recovered with `git log --all --name-only --
     * 'www/content/docs/api/**' 'www/src/app/docs/api/**'`.
     *
     * "Nothing can add an eleventh" was the original claim and it still holds for
     * this group. It was never a claim about the array — group 2 is a different
     * set from a different cause, so it is listed separately rather than appended
     * here, which would have made this paragraph false.
     *
     * The four `/admin/*` operations are absent deliberately. They are filtered
     * out of the public `/openapi.json`, so there is no page to send anyone to,
     * and a redirect would be the one place still advertising that they exist.
     *
     * The section root moved from `/api` to `/reference` — see `openapi.route`.
     * That first rule carries every inbound link to `/docs/api`, including the one
     * in the footer of every page this site has served. */
    { from: '/api', to: '/reference', status: 302 },
    { from: '/api/get-trace', to: '/reference', status: 302 },
    { from: '/api/investigate-services', to: '/reference', status: 302 },
    { from: '/api/list-anomalies', to: '/reference', status: 302 },
    { from: '/api/list-services', to: '/reference', status: 302 },
    { from: '/api/run-query', to: '/reference', status: 302 },
    { from: '/api/v1/anomalies/get', to: '/reference', status: 302 },
    { from: '/api/v1/investigate/post', to: '/reference', status: 302 },
    { from: '/api/v1/query/post', to: '/reference', status: 302 },
    { from: '/api/v1/services/get', to: '/reference', status: 302 },
    { from: '/api/v1/traces/trace_id/get', to: '/reference', status: 302 },

    /* GROUP 2 — the three flat guide URLs, retired when the docs were sorted into
     * Diátaxis sections (tutorial / how-to / reference / explanation).
     *
     * The pages did not change identity, only mode: the quickstart is the tutorial
     * and the other two are how-to guides, so each old URL has exactly one new
     * home and the mapping is 1:1. Slugs are deliberately unchanged, so this list
     * is a directory rewrite and nothing else.
     *
     * `.md` and `.mdx` are listed as well because they are real served URLs, not
     * an implementation detail. Blume writes a raw-Markdown variant beside every
     * page, `_headers` gives `/docs/*.md` a `text/markdown` content type, and each
     * page advertises its own variant in a `Link: rel="alternate"` header. An
     * agent that kept `/docs/quickstart.md` is exactly the reader this product is
     * for, so the extensionless form alone would have covered browsers and missed
     * them. */
    { from: '/quickstart', to: '/tutorials/quickstart', status: 302 },
    { from: '/quickstart.md', to: '/tutorials/quickstart.md', status: 302 },
    { from: '/quickstart.mdx', to: '/tutorials/quickstart.mdx', status: 302 },
    { from: '/install-with-ai', to: '/how-to/install-with-ai', status: 302 },
    { from: '/install-with-ai.md', to: '/how-to/install-with-ai.md', status: 302 },
    { from: '/install-with-ai.mdx', to: '/how-to/install-with-ai.mdx', status: 302 },
    { from: '/hosted-onboarding', to: '/how-to/hosted-onboarding', status: 302 },
    { from: '/hosted-onboarding.md', to: '/how-to/hosted-onboarding.md', status: 302 },
    { from: '/hosted-onboarding.mdx', to: '/how-to/hosted-onboarding.mdx', status: 302 },
  ],

  /**
   * `robots: false` because ours is a real static asset at `public/robots.txt`.
   *
   * Blume's generator writes to `deployStaticDir()`, which on a Cloudflare server
   * build is `dist/` — one level above `dist/client`, the only directory the Worker
   * serves. So its copy was never reachable, and turning the generator off is the
   * native way to say "this file is ours" instead of letting it write a file that
   * nothing can read. Ours also declares the AI crawlers explicitly, which is a
   * statement worth keeping in a file a human edits.
   *
   * CARRIES A DEBT, and it shipped broken once: this flag also gates the
   * `Content-Signal:` line, which `seo.contentSignals` (default: all-yes) puts in
   * Blume's robots.txt and nowhere else. Turning `robots` off dropped it while
   * `agent-readability.json` went on declaring the same policy in `contentUsage`,
   * so the manifest and the file a crawler reads for that disagreed. The line is
   * now written by hand in `public/robots.txt`. Nothing checks that the two agree —
   * if `contentSignals` is ever configured away from its default, that file has to
   * be edited in the same commit.
   *
   * `sitemap` deliberately stays ON even though `@astrojs/sitemap` writes the one
   * actually served and Blume's copy is equally unreachable. It is not symmetric
   * with robots: `buildAgentReadability` gates the manifest's sitemap entry on this
   * flag (`if (site && config.seo.sitemap)`), so turning it off would drop
   * `sitemap` from `agent-readability.json` — the file agents read to find
   * everything else — while we do in fact serve one. An inert duplicate in `dist/`
   * costs nothing; a missing advertisement costs a crawler its map.
   */
  seo: { robots: false },

  deployment: {
    site: 'https://sigiro.com',
    // Named rather than left to auto-detection, which keys off CI environment
    // variables and would pick nothing on a laptop — the dev server warned that
    // there was no adapter and a build would fail.
    //
    // `server` and not `static` because the MCP route below is request-time.
    adapter: 'cloudflare',
    output: 'server',

    /**
     * `server` because the injected `/dl/[...key]` route and the MCP endpoint are
     * both request-time. Every HTML route still prerenders.
     *
     * On the artifact-location defect this combination has, and what is and is not
     * done about it, see the `integrations` note above.
     */
  },

  /**
   * The three ink/accent values from www/DESIGN.md, and nothing else.
   *
   * The accent is the one colour that means anything on this brand — §7 — and
   * it flips with the theme. Geist is the pairing the design language is written
   * for. Everything past this is Blume's own theme, on purpose: matching the
   * landing page pixel for pixel was what the 182-line stylesheet did, and it is
   * exactly the custom code this move is meant to delete.
   */
  theme: {
    accent: { light: '#4568FF', dark: '#93B0FF' },
    background: { light: '#EFEEEA', dark: '#141312' },
    // The roles are display/body/mono, and both Geist faces are in Blume's
    // curated set — so the pairing the design language is written for comes
    // across with no self-hosting to arrange.
    fonts: { display: 'geist', body: 'geist', mono: 'geist-mono' },
    mode: 'system',
  },

  /**
   * Blume's own renderer rather than Scalar. Both were available; this one
   * renders as pages in the docs shell, which is what the reference already was,
   * and it needs no second SPA in the bundle.
   *
   * The spec is the one sigiro serves. It is public and unauthenticated by
   * design — `/openapi.json` is the entire discoverability surface — so it is
   * read from the live API rather than copied into this repo, and cannot drift.
   */
  openapi: {
    enabled: true,
    /**
     * `/reference`, not `/api`, and the reason is the label rather than the URL.
     *
     * Blume humanizes the route segment when it has nothing better, and it cannot
     * know `api` is an initialism — the same code renders `sql` as `Sql`, so the
     * sidebar and breadcrumb both read "Api". `openapi.sources[].label` is
     * documented as the nav label for a source and changes neither, so it was
     * tried and reverted. Renaming the segment is the fix that works: `reference`
     * humanizes to "Reference" with nothing to special-case.
     *
     * Checked against what mature references do before picking it. Stripe and
     * Resend both label this section "API Reference"; Resend's slug is literally
     * `api-reference`. Neither exposes the word "OpenAPI" in navigation — that
     * names the file format, which is the publisher's concern and not the
     * reader's.
     *
     * `/docs/api` and every operation path under it keep working: see the
     * `redirects` block above, which now sends the old section root and the ten
     * legacy per-operation URLs here.
     */
    route: '/reference',
    spec: 'https://api.sigiro.com/openapi.json',
    codeSamples: ['curl', 'javascript', 'python'],
  },

  ai: {
    /**
     * Four read-only tools — search_docs, get_page, list_pages, get_navigation
     * — that Claude Code, Cursor and VS Code read directly. This is the reason
     * the move is worth making: everything else on Blume's list we either had
     * already or did not want.
     */
    mcp: {
      enabled: true,
      name: 'sigiro-docs',
      instructions:
        'Documentation for sigiro, an observability data layer. For live telemetry, use the sigiro API at https://api.sigiro.com — its full surface is described at https://api.sigiro.com/openapi.json.',
    },
    /** Generated, including the API reference, replacing two route handlers. */
    llmsTxt: { enabled: true, openapi: true },
    /**
     * `ask` stays off. It needs an API key, a model provider and a per-query
     * bill, and the landing page already hands a reader's own assistant a
     * prefilled prompt pointed at these docs for nothing.
     */
  },

  /** Orama, local, no hosted service and no key. */
  search: { provider: 'orama' },

  github: { owner: 'sigiroai', repo: 'sigiro' },

  navigation: {
    /**
     * Two external links, and the second one names the file rather than a
     * section. That is deliberate here: it leaves the docs for the raw spec on
     * `api.sigiro.com`, so "OpenAPI" is what a reader actually lands on. The
     * generated reference is a docs page and lives in the sidebar as
     * "Reference" — see `openapi.route`.
     */
    featured: [
      { href: 'https://sigiro.com', label: 'Home' },
      { href: 'https://api.sigiro.com/openapi.json', label: 'openapi.json' },
    ],
    /**
     * No explicit `sidebar` here on purpose. Listing the four pages did fix the
     * order, and it also silently dropped the generated API reference from the
     * sidebar — an explicit sidebar is the whole sidebar, and the reference
     * appends itself only to the inferred one. `/api` still rendered, with no
     * way to navigate to it.
     *
     * Order comes from `sidebar.order` in each page's frontmatter instead, which
     * is Blume's own mechanism and leaves inference in charge. Loose pages hoist
     * above groups, so the four guides sort themselves and the reference follows.
     */
  },
});
