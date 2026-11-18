import { defineMeta } from 'blume';

/**
 * Diátaxis mode 4 of 4: informs cognition, serves acquisition of skill.
 *
 * This was the missing mode. Nothing public answered "why is sigiro built this
 * way" — the material existed only in `docs/notes/architecture/*` and
 * `.agents/product-marketing.md`, neither of which ships.
 *
 * Diátaxis names this mode "Explanation" and suggests "Discussion",
 * "Background", "Conceptual guides" or "Topics" as alternatives. "Explanation"
 * is kept because it is the name a reader who knows the framework looks for.
 *
 * Every title in here reads with an implicit "about" in front of it, which is
 * the test Diátaxis sets for this mode.
 */
export default defineMeta({
  order: 4,
  title: 'Explanation',
});
