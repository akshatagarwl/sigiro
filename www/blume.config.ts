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
    'astro:config:setup': ({ addMiddleware, injectRoute, updateConfig }) => {
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
      /**
       * docs.sigiro.com → sigiro.com/docs. Blume owns `srcDir`, so a
       * `src/middleware.ts` here is never picked up on its own — and a
       * `_redirects` rule cannot do it, because Cloudflare matches those on the
       * path with no hostname to test. See the file for the rest.
       */
      addMiddleware({
        order: 'pre',
        entrypoint: fileURLToPath(new URL('./src/middleware.ts', import.meta.url)),
      });
    },
  },
});

/**
 * docs.sigiro.com.
 *
 * This replaced a fumadocs install inside the Next app at sigiro.com/docs. The
 * rule for this file is that everything here is configuration: no custom MDX
 * components, no hand-written page shell, no bespoke API reference. The parts
 * that were custom before — a 177-line operation renderer, a generator script
 * that turned our OpenAPI spec into MDX, two llms.txt route handlers, a 182-line
 * stylesheet mapping fumadocs' variables onto our tokens — are all options
 * below.
 *
 * The one thing genuinely lost in the move is that docs are now a different
 * origin from the landing page, so the two cannot share a `localStorage`
 * preference. That is a browser rule, not a Blume limitation.
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
   * days at docs.sigiro.com; that hostname now redirects here.
   */
  basePath: '/docs',

  /**
   * `sigiro()` is the alias + `/dl` route, above.
   *
   * `@astrojs/sitemap` is here to route around a Blume defect, and it is the fix
   * Blume's own customization guide reaches for — `integrations` is documented
   * with `@astrojs/sitemap` as its example.
   *
   * The defect: Blume writes `sitemap.xml`, `robots.txt`, `llms.txt`,
   * `llms-full.txt` and `agent-readability.json` to `deployStaticDir()`, which
   * returns `dist/client` for `adapter: 'node'` and plain `dist` for
   * `cloudflare`. But `@astrojs/cloudflare` in server mode serves `dist/client`
   * — its generated wrangler config sets `assets.directory: "../client"` — so on
   * Cloudflare every one of those files lands one directory above everything the
   * Worker can see, and 404s. The Cloudflare case belongs in the same branch as
   * node; that is a one-line upstream fix and it is filed rather than patched
   * here.
   *
   * What is native rather than a workaround: an Astro *integration* writes into
   * Astro's own client output, so its file is served. `@astrojs/sitemap` is a
   * real sitemap generator that happens to put the file in the right place, and
   * `public/robots.txt` is a real static asset. Both beat the copy-after-build
   * they replace, and neither reaches into Blume's output.
   *
   * Blume still writes its own unreachable copies into `dist/`. They are inert —
   * ours are the ones served — and they disappear when the upstream fix lands.
   *
   * NOT fixed by this: `llms.txt`, `llms-full.txt` and `agent-readability.json`.
   * Those have no Astro-native generator to substitute — they are Blume's own
   * content pipeline, and `buildLlmsFiles` needs a build-time `BlumeProject` that
   * an Astro route cannot get hold of. They wait on upstream.
   */
  integrations: [sigiro(), sitemap()],

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
     * KNOWN DEFECT, unfixed on purpose — raise it before working around it.
     *
     * With this combination, `llms.txt`, `llms-full.txt`, `sitemap.xml`,
     * `robots.txt` and `agent-readability.json` are generated but never served.
     * They 404 in production. (`/robots.txt` answers 200, which is Cloudflare's
     * own managed file, not ours — worth knowing, because it hides the failure.)
     *
     * Blume writes them to `deployStaticDir()`, whose comment states the
     * assumption plainly: "Netlify publishes `dist/` itself and Cloudflare serves
     * the `outDir` root". That is not true of `@astrojs/cloudflare` v14 in server
     * mode, which serves `dist/client` — its generated wrangler.json sets
     * `assets.directory: "../client"`. So the files land one directory above
     * everything the Worker can see.
     *
     * The obvious fix is a `cp` in the build script. It would work and it would
     * be a makeshift patch over an upstream bug, so it is not here.
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
    route: '/api',
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
    featured: [
      { href: 'https://sigiro.com', label: 'Home' },
      { href: 'https://api.sigiro.com/openapi.json', label: 'OpenAPI' },
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
