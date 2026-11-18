import { defineMeta } from 'blume';

/**
 * Diátaxis mode 2 of 4: informs action, serves application of skill.
 *
 * `title` is set because the directory name does not humanize to the label we
 * want. Blume splits a segment on `-`/`_` and capitalizes each piece, so
 * `how-to` renders as "How To". Diátaxis names this mode "How-to guides", and
 * the reader recognizes that name from every other project that uses the
 * framework.
 *
 * See `../tutorials/meta.ts` for why the ordering lives here rather than in
 * `navigation.sidebar`.
 */
export default defineMeta({
  order: 2,
  title: 'How-to guides',
});
