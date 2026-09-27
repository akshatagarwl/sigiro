import { defineMeta } from "blume";

/**
 * Diátaxis mode 3 of 4: informs cognition, serves application of skill.
 *
 * Most of this section is GENERATED from `https://sigiro.com/openapi.json` by
 * `openapi.route: '/reference'` in `blume.config.ts`. Those pages arrive through
 * a staged content source, so they are real Blume pages with real routes, and the
 * sidebar group `reference` is built from them. `cli.md` is the one hand-written
 * page here: the CLI is not in the OpenAPI document, so nothing generates it.
 *
 * A staged page cannot carry a `meta.ts` of its own, and its group would
 * therefore sort at `+Infinity` — after every section that names an order.
 * Diátaxis puts Reference third, between How-to guides and Explanation. This
 * file is the only way to say so: `discoverFolderMeta` globs `**\/meta.ts` under
 * the content root and keys each hit by its directory, and `applyFolderMeta`
 * then decorates the group that the staged pages built.
 *
 * `title` is deliberately ABSENT. The generated overview page carries its own
 * frontmatter title (the spec's `info.title`), and Blume warns
 * (BLUME_NAV_INDEX_TITLE_MISMATCH) when an index page's title disagrees with its
 * folder's `meta.title`. The directory name already humanizes to "Reference",
 * which is the label we want, so there is nothing to override and nothing to
 * disagree with.
 *
 * Check the route before you add another hand-written page here. The generated
 * routes are `/docs/reference` and `/docs/reference/operations/*`; any other slug
 * is free, but a collision silently drops a whole spec's pages
 * (BLUME_OPENAPI_ROUTE_COLLISION).
 */
export default defineMeta({
  order: 3,
});
