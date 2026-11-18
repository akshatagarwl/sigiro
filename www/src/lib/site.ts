/**
 * One source of truth for everything the site says about itself, assembled per
 * locale.
 *
 * This file used to hold both the copy and the artifacts. It was split when the
 * landing page gained Chinese, Japanese and Korean versions, because Blume
 * cannot translate it: `blume translate` walks the content graph
 * (`work-list.ts` iterates `project.graph.pages`, whose universe is
 * "filesystem-backed default-locale pages"), and `pages/index.astro` is an
 * Astro component rather than a page record. Blume's own docs site has the same
 * gap — it ships four content locales and an English-only `pages/index.astro`.
 *
 * The split is by *risk*, not by tidiness:
 *
 * - `site.shared.ts` — shell commands, terminal output, icon slugs, URLs, the
 *   `ours` flags, the analytics token. A translation must never reach these: a
 *   localized `docker run` or a translated `SELECT` is a page that lies about
 *   what the product does, and nobody here reads all three target languages
 *   well enough to catch it in review.
 * - `site.<locale>.ts` — prose only. Each implements the `LandingCopy` type
 *   derived from `site.en.ts`, so a missing field is a compile error rather than
 *   an `undefined` rendered into the page.
 *
 * The exports below keep the exact names and shapes they had before the split,
 * so the ten files importing them did not change. `getCopy(locale)` is the only
 * new entry point, used by `pages/[locale]/index.astro`.
 */

import { EN, type LandingCopy } from './site.en';
import { JA } from './site.ja';
import { KO } from './site.ko';
import {
  CALL_HREF,
  DISCOVER_ARTIFACT,
  HOW_SESSIONS,
  INTEGRATION_ITEMS,
  type LandingLocale,
  type Locale,
  LOOP_SESSIONS,
  NAV_HREFS,
  READ_ALL_THREE,
  SITE_SHARED,
} from './site.shared';
import { ZH } from './site.zh';

export {
  CF_ANALYTICS_TOKEN,
  LANDING_LOCALES,
  type LandingLocale,
  type Locale,
  LOCALE_LABELS,
  SOCIAL,
} from './site.shared';

const COPY: Record<Locale, LandingCopy> = { en: EN, ja: JA, ko: KO, zh: ZH };

/**
 * Everything the landing page renders, for one locale.
 *
 * Joins the locale's prose to the shared artifacts. The returned shape matches
 * the module-level exports below one for one, so a component can take either —
 * `pages/index.astro` uses the exports (English), `pages/[locale]/index.astro`
 * calls this.
 */
export const getCopy = (locale: Locale) => {
  const c = COPY[locale];

  return {
    site: {
      ...SITE_SHARED,
      title: c.title,
      tagline: c.tagline,
      description: c.description,
    },
    how: [
      { title: c.how.connect.title, say: c.how.connect.say, ...HOW_SESSIONS.connect },
      { title: c.how.ask.title, say: c.how.ask.say, ...HOW_SESSIONS.ask },
      { title: c.how.correlate.title, say: c.how.correlate.say, ...HOW_SESSIONS.correlate },
      { title: c.how.prove.title, say: c.how.prove.say, ...HOW_SESSIONS.prove },
    ],
    sections: [
      {
        n: c.discover.n,
        lead: c.discover.lead,
        rest: c.discover.rest,
        say: c.discover.say,
        artifact: {
          title: c.discover.artifactTitle,
          meta: c.discover.artifactMeta,
          ...DISCOVER_ARTIFACT,
        },
      },
    ],
    loop: [
      { ...c.loop.detect, ...LOOP_SESSIONS.detect },
      { ...c.loop.diagnose, ...LOOP_SESSIONS.diagnose },
      { ...c.loop.fix, ...LOOP_SESSIONS.fix },
      { ...c.loop.verify, ...LOOP_SESSIONS.verify },
      { ...c.loop.learn, ...LOOP_SESSIONS.learn },
    ],
    promises: c.promises,
    /* The strings that used to be hardcoded in the markup: the hero button, the
       section headings, and the two ARIA labels. Forwarded verbatim — the
       components read them straight off `copy.ui`. */
    ui: c.ui,
    integrations: [
      { label: c.integrations.emitLabel, items: INTEGRATION_ITEMS.emit },
      { label: c.integrations.runLabel, items: INTEGRATION_ITEMS.run },
      {
        label: c.integrations.readLabel,
        /* The one prose entry in an otherwise all-brand list, appended here so
           the locale file owns the sentence and the shared file owns the marks. */
        items: [
          ...INTEGRATION_ITEMS.read,
          { name: c.integrations.readAnySql, icon: null },
        ],
      },
    ],
    nav: [
      { href: NAV_HREFS.docs, label: c.nav.docs },
      { href: NAV_HREFS.reference, label: c.nav.reference },
    ],
    call: { href: CALL_HREF, label: c.callLabel },
    /* The three URLs are prepended from the shared constant rather than written
       into any locale file, so a translation cannot corrupt a path. The
       assistant reads the URLs regardless of the language around them. */
    askAiPrompts: c.askAi.map((q) => `${READ_ALL_THREE} ${q}`),
  };
};

/**
 * The English exports, unchanged in name and shape from before the split.
 *
 * `pages/index.astro` and its components import these directly, so the English
 * page's render path is exactly what it was — which is the property that makes
 * this refactor reviewable: if `/` changes at all, the split is wrong.
 */
const en = getCopy('en');

export const SITE = en.site;
export const HOW = en.how;
export const SECTIONS = en.sections;
export const LOOP = en.loop;
export const PROMISES = en.promises;
export const INTEGRATIONS = en.integrations;
export const NAV = en.nav;
export const CALL = en.call;
export const ASK_AI_PROMPTS: readonly string[] = en.askAiPrompts;
