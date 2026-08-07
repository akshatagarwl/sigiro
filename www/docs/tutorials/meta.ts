import { defineMeta } from 'blume';

/**
 * Diátaxis mode 1 of 4: informs action, serves acquisition of skill.
 *
 * `order` here — and in the three sibling `meta.ts` files — is the ONLY thing
 * that fixes the section sequence. It cannot be `navigation.sidebar` in
 * `blume.config.ts`: an explicit sidebar replaces the whole tree, and the
 * generated API reference appends itself only to the INFERRED one, so listing
 * the sections there deletes `/docs/reference` from the sidebar. That was tried
 * and reverted once already; the note on `navigation` in `blume.config.ts`
 * records it. Folder meta decorates the inferred tree instead
 * (`buildFileSystemSidebar` -> `applyFolderMeta`), so inference stays in charge
 * and the reference keeps appending itself.
 *
 * Page order inside the section comes from `sidebar.order` in each page's own
 * frontmatter, not from a `pages` list here. One mechanism per axis: meta
 * orders sections, frontmatter orders pages.
 *
 * ONE ACCEPTED DEVIATION from Diátaxis lives in this section. `tutorials.rst`
 * says a tutorial ignores options and alternatives, because a choice is
 * cognitive work the learner did not ask for. `quickstart.mdx` still offers a
 * binary/docker tab group. The reason is that the same group is `sync`ed with
 * the landing page's install block, so one pick follows the reader across the
 * whole site and `?install=docker` is a shareable link. Removing the choice here
 * would split that preference in two and break the link. The choice is also made
 * once and then remembered, so the learner is not asked twice.
 */
export default defineMeta({
  order: 1,
  title: 'Tutorials',
});
