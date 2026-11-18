import { defineComponents } from 'blume';

/**
 * Blume layout slots for the docs pages.
 *
 * `blume.config.ts` says everything about the docs is configuration and nothing
 * is custom. This file is the one exception, and it exists for a specific gap
 * rather than for styling: the landing page's shell is `src/layouts/Base.astro`,
 * which mounts our header and footer itself, but Blume renders `/docs/*` with
 * its own shell. Its `Footer` slot has no built-in — it renders nothing until
 * set — so a reader who installed sigiro, got it running and formed an opinion
 * had no way to reach anybody from the page they were on.
 *
 * `Header` is deliberately NOT overridden. Blume's docs header carries the
 * search trigger, the sidebar toggle and the mobile-drawer wiring; replacing it
 * to match the landing page's bar would mean reimplementing those or losing
 * search. The two-shell split is the trade `blume.config.ts` already documents.
 *
 * The slot passes `site`, `navigation` and `ui`. Footer.astro takes no props and
 * reads its links from `src/lib/site.ts`, so the site keeps one source of truth
 * for what the footer says on both surfaces.
 */
export default defineComponents({
  layout: {
    Footer: './src/components/site/Footer.astro',
  },
});
