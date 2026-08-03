import type { MiddlewareHandler } from 'astro';

/**
 * docs.sigiro.com → sigiro.com/docs.
 *
 * The docs lived on that subdomain for two days. They are `/docs/*` on the apex
 * again, and one Worker answers on both hostnames — so the subdomain has to be
 * retired rather than merely stop being linked: it is in the README, in the nav of
 * anything cached, and in whatever an agent stored from `llms.txt`.
 *
 * This cannot be a `_redirects` rule. Cloudflare matches those on the path only —
 * sources are root-relative and carry no hostname — so a file rule cannot tell
 * `docs.sigiro.com/quickstart` from `sigiro.com/quickstart`. Astro middleware sees
 * the whole URL, and `addMiddleware` from `blume.config.ts` is the supported way in
 * (Blume owns `srcDir`, so a `src/middleware.ts` in this project is never picked up
 * on its own).
 *
 * 301, unlike everything in `_redirects`. Those are 302 because they point at
 * third-party accounts that can move; this is our own hostname being retired for
 * good, the pages' canonical tags already name the apex, and a permanent signal is
 * what transfers the two days of indexing rather than splitting it.
 *
 * Path mapping is a straight prefix: the subdomain's root was the docs root, so
 * `docs.sigiro.com/quickstart` → `sigiro.com/docs/quickstart`. `/llms.txt` and the
 * other agent files are the exception — they are site-root conventions, and on the
 * apex they live at the root rather than under `/docs`.
 */
const ROOT_FILES = new Set([
  '/llms.txt',
  '/llms-full.txt',
  '/agent-readability.json',
  '/robots.txt',
  '/sitemap.xml',
  '/sitemap-index.xml',
]);

export const onRequest: MiddlewareHandler = (context, next) => {
  const url = context.url;

  if (url.hostname !== 'docs.sigiro.com') return next();

  const target = new URL(url);
  target.hostname = 'sigiro.com';
  if (!ROOT_FILES.has(url.pathname)) {
    // `/` → `/docs`, `/quickstart` → `/docs/quickstart`.
    target.pathname = `/docs${url.pathname === '/' ? '' : url.pathname}`;
  }

  return context.redirect(target.href, 301);
};
