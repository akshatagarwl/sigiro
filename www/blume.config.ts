import { fileURLToPath } from "node:url";

import sitemap from "@astrojs/sitemap";
import type { AstroIntegration } from "astro";
import { defineConfig } from "blume";

import { CF_ANALYTICS_TOKEN, SITE } from "./src/lib/site";

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
  name: "sigiro",
  hooks: {
    "astro:config:setup": ({ injectRoute, injectScript, updateConfig }) => {
      updateConfig({
        vite: {
          resolve: {
            alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
          },
          /**
           * `blume dev` crashed on startup with
           *
           *   Error: The file does not exist at ".blume/.cache/vite/deps_ssr/
           *   sequence-<hash>.js" which is in the optimize deps directory.
           *
           * `sequence-*.js` is not a package — it is a chunk Vite's SSR
           * dependency optimizer emits. The log line before the crash names the
           * trigger:
           *
           *   [vite] dependency optimized: astro/app/manifest
           *   [vite] optimized dependencies changed. reloading
           *
           * Astro's Cloudflare adapter runs SSR inside workerd. The optimizer
           * discovers `astro/app/manifest` late, re-optimizes, and rewrites
           * `deps_ssr/` with fresh hashes while the workerd runner still holds a
           * handle to the previous chunk. The runner asks for a file that was
           * just replaced and the dev server dies instead of reloading.
           *
           * Naming it up front makes the first optimizer pass the only pass, so
           * nothing is invalidated underneath the runner. The React packages are
           * listed for the same reason: the auth island pulls them in through a
           * `client:load` component, which is also late discovery.
           *
           * `astro build` optimizes once by construction, which is why the site
           * deployed fine the whole time the dev server could not start.
           */
          ssr: {
            optimizeDeps: {
              include: [
                "astro/app/manifest",
                "motion",
                "motion/react",
                "react",
                "react-dom",
                "react-dom/server",
              ],
            },
          },
        },
      });
      injectRoute({
        pattern: "/dl/[...key]",
        entrypoint: fileURLToPath(new URL("./src/dl.ts", import.meta.url)),
      });
      injectScript("page", "import '../src/webmcp/browser.ts';");
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
  title: "sigiro",
  /**
   * Imported, not copied. This string reaches `llms.txt`, `llms-full.txt`, every
   * `/docs/**` page and `agent-readability.json` — the artifacts an agent reads —
   * while `SITE.description` reaches the landing page. A hand-duplicated copy
   * drifted: the landing page dropped a causal claim the docs contradict, and
   * this file kept serving the old sentence to every agent surface. One constant
   * makes that failure impossible. `site.ts` exports a plain object and runs no
   * side effects, so it is safe for a config that evaluates twice per run.
   */
  description: SITE.description,

  logo: { text: "sigiro", href: "/" },

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
  basePath: "/docs",

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
   * a 301 is cached by the browser more or less forever. Every entry below used to
   * need a matching negative `run_worker_first` rule in `wrangler.jsonc` to hold
   * that status and its query string at the edge. Blume now bakes a redirect table
   * into the generated wrapper Worker and applies `_redirects` semantics to the
   * `Location` header, so this list is self-sufficient and no longer has a twin to
   * keep in sync. The note in `wrangler.jsonc` records the three upstream changes
   * that took.
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
     * The `/admin/*` operations are absent deliberately. They are filtered
     * out of the public `/openapi.json`, so there is no page to send anyone to,
     * and a redirect would be the one place still advertising that they exist.
     *
     * The section root moved from `/api` to `/reference` — see `openapi.route`.
     * That first rule carries every inbound link to `/docs/api`, including the one
     * in the footer of every page this site has served. */
    { from: "/api", to: "/reference", status: 302 },
    { from: "/api/get-trace", to: "/reference", status: 302 },
    { from: "/api/list-services", to: "/reference", status: 302 },
    { from: "/api/run-query", to: "/reference", status: 302 },
    { from: "/api/v1/query/post", to: "/reference", status: 302 },
    { from: "/api/v1/services/get", to: "/reference", status: 302 },
    { from: "/api/v1/traces/trace_id/get", to: "/reference", status: 302 },

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
    { from: "/quickstart", to: "/tutorials/quickstart", status: 302 },
    { from: "/quickstart.md", to: "/tutorials/quickstart.md", status: 302 },
    { from: "/quickstart.mdx", to: "/tutorials/quickstart.mdx", status: 302 },
    { from: "/install-with-ai", to: "/how-to/install-with-ai", status: 302 },
    {
      from: "/install-with-ai.md",
      to: "/how-to/install-with-ai.md",
      status: 302,
    },
    {
      from: "/install-with-ai.mdx",
      to: "/how-to/install-with-ai.mdx",
      status: 302,
    },
    {
      from: "/hosted-onboarding",
      to: "/how-to/hosted-onboarding",
      status: 302,
    },
    {
      from: "/hosted-onboarding.md",
      to: "/how-to/hosted-onboarding.md",
      status: 302,
    },
    {
      from: "/hosted-onboarding.mdx",
      to: "/how-to/hosted-onboarding.mdx",
      status: 302,
    },
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

  /**
   * Chinese, Japanese and Korean, and deliberately not the European locales.
   *
   * Picked from prior art rather than taste. Of the comparable projects checked
   * on 2026-08-12, OpenTelemetry ships zh/ja/ko/fr/es/pt and another mature project ships
   * zh/ja/ko/fr/es — the exact intersection is zh, ja, ko. Neither translates German,
   * which is the assumption this measurement overturned.
   *
   * The three chosen are where translation actually earns its keep: developers in
   * France, Spain and Germany overwhelmingly search technical queries in English,
   * so a French page competes for a query few French developers type. Chinese is
   * the strongest case on its own — Baidu and the Chinese developer web are a
   * largely separate index that English pages do not reach at all.
   *
   * Both mature projects use bare `zh`, not `zh-cn`. Followed here.
   *
   * A second reason the European locales lose: Cloudflare Web Analytics on the
   * free plan excludes visitors served from EU/EEA data centres by default, so
   * those are precisely the locales whose readership we could not measure. The
   * three chosen are fully measured.
   *
   * NOTE ON EVIDENCE. This choice rests on prior art, not on our own traffic —
   * analytics went live the same day and had collected nothing when these locales
   * were picked. Revisit once there is a month of country data: if it contradicts
   * this, the ledger makes adding or dropping a locale cheap.
   *
   * `style` is set on each because a first translation has no existing page to
   * match register against, so the choice has to be pinned up front. It rides
   * along in every translation prompt, and it wins over an existing translation
   * that disagrees — so it also nudges older pages on a retranslation.
   */
  i18n: {
    defaultLocale: "en",
    locales: [
      { code: "en", label: "English" },
      /* Simplified, mainland conventions. Technical register — this reader is
         installing a binary, not reading marketing. */
      {
        code: "zh",
        label: "简体中文",
        style: "Simplified Chinese, mainland technical register",
      },
      /* です/ます rather than plain form: the standard register for Japanese
         software documentation. */
      {
        code: "ja",
        label: "日本語",
        style: "Polite です/ます form, technical documentation register",
      },
      /* 합니다 style, the formal-polite register Korean technical docs use. */
      {
        code: "ko",
        label: "한국어",
        style: "Formal 합니다 style, technical documentation register",
      },
    ],
  },

  /**
   * Cloudflare Web Analytics, as a `scripts` entry rather than the dashboard's
   * automatic injection.
   *
   * Free on every Cloudflare plan, and the only site-limit that exists applies to
   * sites *not* proxied through Cloudflare — this one is, so there is no cap. The
   * free tier's "0 rules" limit means the snippet cannot be scoped to specific
   * subdomains, which is not a constraint for a single property.
   *
   * NOT the dashboard's automatic setup, and the reason is a documented failure
   * mode rather than a preference. Automatic injection works by having the
   * Cloudflare proxy rewrite the HTML payload in flight; Cloudflare's own docs
   * warn it silently does not happen when the proxy cannot modify the payload
   * (their stated example is `Cache-Control: public, no-transform`). This site
   * serves `/`, `/docs` and `/docs/*` from a Worker — see `run_worker_first` in
   * `wrangler.jsonc` — which is exactly the shape where an in-flight rewrite is
   * fragile, and the failure is indistinguishable from "nobody visited". Emitting
   * the tag at build time removes that dependency: the beacon is in the HTML
   * because this config put it there, and `dist/client` can be grepped to prove
   * it before a deploy.
   *
   * The dashboard site must therefore be set to "Enable with JS Snippet
   * installation". Leaving it on the automatic setting runs both mechanisms and
   * double-counts every pageview.
   *
   * The token is imported rather than written here, because this block covers
   * `/docs/*` only. The landing page uses its own shell (`src/layouts/Base.astro`)
   * and never renders Blume's `RootLayout`, so it emits the beacon itself from the
   * same constant — see `CF_ANALYTICS_TOKEN` in `src/lib/site.ts`, which carries
   * the reasoning and the note that the token is public rather than secret.
   *
   * Blume emits analytics on `blume build` only, never `blume dev`, so laptop
   * traffic never reaches the dashboard and there is no separate dev property to
   * keep.
   *
   * Why this exists at all: the site shipped with no analytics of any kind, so
   * questions like "which locales are worth translating into" had no data behind
   * them. Country and language breakdown is the signal that turns that from a
   * guess into a measurement. Note for that decision specifically — on the free
   * plan the RUM beacon excludes visitors served from EU/EEA data centres by
   * default, so EU locales are the ones this will under-report.
   */
  analytics: {
    scripts: [
      {
        src: "https://static.cloudflareinsights.com/beacon.min.js",
        strategy: "defer",
        attributes: {
          "data-cf-beacon": `{"token": "${CF_ANALYTICS_TOKEN}"}`,
        },
      },
    ],
  },

  deployment: {
    site: "https://sigiro.com",
    // Named rather than left to auto-detection, which keys off CI environment
    // variables and would pick nothing on a laptop — the dev server warned that
    // there was no adapter and a build would fail.
    //
    // `server` and not `static` because the MCP route below is request-time.
    adapter: "cloudflare",
    output: "server",

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
    accent: { light: "#4568FF", dark: "#93B0FF" },
    background: { light: "#EFEEEA", dark: "#141312" },
    // The roles are display/body/mono, and both Geist faces are in Blume's
    // curated set — so the pairing the design language is written for comes
    // across with no self-hosting to arrange.
    fonts: { display: "geist", body: "geist", mono: "geist-mono" },
    mode: "system",
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
    route: "/reference",
    spec: "https://sigiro.com/openapi.json",
    codeSamples: ["curl", "javascript", "python"],

    /**
     * OFF. 1.5.2 added a "Try it" panel to every operation page and defaulted it
     * on with the Blume renderer; on this spec it cannot complete a single
     * request, so it is an affordance that fails for every reader.
     *
     * The live spec at `sigiro.com/openapi.json` declares no `servers`
     *      block, and Blume's playground model is `servers: args.servers.map(…)`
     *      with `server: model.servers[0] ?? ""`. The built page carries
     *      `"servers":[]`, so the panel renders only its "Custom base URL"
     *      field — placeholder `https://api.example.com` — and a reader who
     *      presses Send without typing one sends to nothing.
     *
     * Nothing is lost against 1.4.3, which had no playground at all. The
     * per-operation `codeSamples` above are unchanged and remain copy-pasteable,
     * which is the affordance that actually works against an API that requires
     * a token. Revisit if the API ever declares `servers`.
     */
    playground: false,
  },

  ai: {
    /**
     * OFF, and this is the one setting on this site that a Blume upgrade forced
     * rather than offered.
     *
     * 1.6.2 gave every site a read-only JSON docs API under `/api/docs/` and
     * described it with a generated OpenAPI 3.1 document at `/openapi.json`.
     * That path is already the telemetry API's own description: the website
     * Worker proxies `/openapi.json` to the API origin, and
     * `public/.well-known/integrations.json` declares it as the canonical spec.
     *
     * Leaving `api` on would publish a *different* document at that path. Three
     * surfaces advertise the generated document as this site's machine-readable
     * service description, all verified in `dist/client` on 1.6.3:
     *
     *   - `agent-readability.json` → `"openapi": "https://sigiro.com/openapi.json"`
     *   - `.well-known/api-catalog` → a `service-desc` on anchor `/api/docs`
     *   - the homepage `Link:` header → `rel="service-desc"`
     *
     * So an agent that reads any of those three is sent to a URL that either
     * hand it the telemetry spec while calling it the docs API, or shadow the
     * API proxy with a static file. Both are false statements about the service.
     *
     * MOVING IT WAS THE PREFERRED FIX AND IS NOT POSSIBLE. `OPENAPI_PATH` is a
     * module constant in `blume/src/ai/api/paths.ts` (`"/openapi.json"`), and
     * `planApi` reads it directly; no config key relocates it. The only other
     * escape hatch 1.6.2 documents is shipping a `public/openapi.json`, which
     * this repo deliberately does not have — the API owns its description — and
     * which would shadow the proxy with a static file rather than remove the
     * conflict.
     *
     * What is given up is a REST twin of tools we already serve. `ai.mcp` below
     * exposes the same page snapshot through the same four operations, and it is
     * the surface `integrations.json` already declares. `llms.txt`,
     * `llms-full.txt` and the per-page `.md` mirrors are unaffected: they are
     * gated on their own flags, not on this one.
     */
    api: false,
    /** The site owns one canonical eight-tool WebMCP catalog; do not also emit Blume's four-tool shim. */
    webmcp: false,
    /**
     * Four read-only tools — search_docs, get_page, list_pages, get_navigation
     * — that Claude Code, Cursor and VS Code read directly. This is the reason
     * the move is worth making: everything else on Blume's list we either had
     * already or did not want.
     */
    mcp: {
      enabled: true,
      route: "/mcp/docs",
      name: "sigiro-docs",
      instructions:
        "Documentation for sigiro, an observability data layer. For live telemetry, use the sigiro API at https://sigiro.com — its full surface is described at https://sigiro.com/openapi.json.",
    },
    /** Generated, including the API reference, replacing two route handlers. */
    llmsTxt: { enabled: true, openapi: true },
    /**
     * ONE skill, published at `/.well-known/agent-skills/`. Not zero-config:
     * `skills` has no default in Blume's schema, so the endpoint 404s until
     * this line names a directory that exists.
     *
     * THE PATH ESCAPES `www/`, DELIBERATELY. `ai.skills` resolves against the
     * project root, which is this directory, so `"skills"` would be the
     * frictionless answer — and it would put the skill inside the website
     * package, which is the one place it does not belong. The same SKILL.md
     * has four consumers: this endpoint, `npx skills add`, the Cursor plugin
     * manifest and the Claude Code plugin manifest. Three of those have
     * nothing to do with the site, and a copy per channel is four texts that
     * drift. Repo-root `skills/` is the pinned public integration repository;
     * its nested `skills/` directory is the layout `npx skills add` consumes.
     * Reading through the gitlink leaves one editable, public source.
     *
     * WHAT THE SKILL IS ALLOWED TO SAY. It restates neither the endpoint list
     * (`sigiro.com/openapi.json` describes it, and a skill that duplicates
     * a spec is tokens without capability) nor the install and onboarding
     * pages (it links `/docs/how-to/install-with-ai` and
     * `/docs/how-to/hosted-onboarding`). What it carries is the ordering the
     * spec cannot state — services, then diagnose, then a finding's
     * `drill_down_sql` posted verbatim to `/v1/query` — and the triage for an
     * empty answer, where the same silence means a broken setup, an SDK that
     * never connected, or a genuinely quiet service.
     *
     * Blume publishes a lone `SKILL.md` verbatim and a skill with supporting
     * files as a `.tar.gz`; this one is a single file on purpose, so it is
     * readable at its URL without unpacking. `llms.txt` gains an "Agent skills"
     * section from the same collection pass, so the two cannot disagree.
     */
    skills: "../skills/skills",
    /**
     * `ask` stays off. It needs an API key, a model provider and a per-query
     * bill, and the landing page already hands a reader's own assistant a
     * prefilled prompt pointed at these docs for nothing.
     */
  },

  /** Orama, local, no hosted service and no key. */
  search: { provider: "orama" },

  /**
   * No `github` key, deliberately. It drove two surfaces and both were wrong:
   * an "Edit this page" link on all 15 docs pages pointing at
   * `github.com/sigiroai/sigiro/edit/main/docs/<path>`, and a `repository`
   * field in `agent-readability.json`. Verified 2026-08-08: the `sigiroai` org
   * has zero public repos and `akshatagarwl/sigiro` is private too, so both
   * targets 404 to every reader. The path was wrong independently of that —
   * published docs live at `www/docs/`, not repo-root `docs/`, which holds
   * internal notes and plans.
   *
   * A broken affordance on every page is worse than no affordance, and it
   * advertised a source location we do not publish. Restore this when the repo
   * is public, with the correct `www/docs` path.
   */

  navigation: {
    /**
     * One entry, and it names the file rather than a section. That is
     * deliberate: it leaves the docs for the raw spec on `sigiro.com`, so
     * "OpenAPI" is what a reader actually lands on. The generated reference is
     * a docs page and lives in the sidebar as "Reference" — see `openapi.route`.
     *
     * No "Home" entry. It read `https://sigiro.com`, which hardcoded this
     * site's own origin into sidebar chrome on all 68 built pages: on a
     * `*.workers.dev` preview every reader who clicked it left the preview for
     * production. It cannot be written root-relative either — blume rebases
     * internal `featured` hrefs under `basePath` and then localizes them
     * (`core/navigation.ts` `rebaseHref`, `core/graph.ts` `localizeHref`), so
     * `/` renders as `/docs`, i.e. the docs index rather than the landing page.
     * The site root is not expressible here.
     *
     * It was redundant regardless: `logo.href` is `/`, and the header brand
     * link renders on every docs page — verified in `dist/client` that
     * `/docs`, `/docs/how-to/executor`, `/docs/ja/reference` and
     * `/docs/reference/operations/run-query` all carry `href="/"` on the
     * wordmark. Two links to one destination, one of them wrong.
     */
    featured: [
      { href: "https://sigiro.com/openapi.json", label: "openapi.json" },
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
