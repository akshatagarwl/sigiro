/**
 * Light or dark, as a store the toggle subscribes to.
 *
 * This was `next-themes`, which was the right dependency under Next: it shipped a
 * pre-paint script, a provider and a hook, and the provider was rendered by the
 * root layout — above everything, before paint. Astro has no React root. A
 * provider here would live inside the toggle's island and its script would run
 * when that island hydrated, which is after paint, which is the flash the script
 * exists to prevent. So the pre-paint half moved into `layouts/Base.astro` as an
 * inline script, and next-themes was left holding one hook.
 *
 * The two behaviours that made it worth depending on are kept below, because they
 * are the two a hand-rolled version usually misses: the operating system changing
 * theme *while the page is open*, and the same choice made in another tab.
 *
 * `localStorage` can throw — Safari in private mode, storage blocked by policy. A
 * missing preference is just "follow the OS", so failing soft is correct.
 */

/** Unchanged from the next-themes install, so nobody's saved choice is dropped. */
const KEY = 'sigiro-theme';

export type Theme = 'light' | 'dark';

function stored(): Theme | null {
  try {
    const value = localStorage.getItem(KEY);
    return value === 'light' || value === 'dark' ? value : null;
  } catch {
    return null;
  }
}

const os = () => matchMedia('(prefers-color-scheme: dark)');

/** A stored choice wins; with none, the operating system decides. */
export function resolved(): Theme {
  return stored() ?? (os().matches ? 'dark' : 'light');
}

const watchers = new Set<() => void>();

/**
 * The class on `<html>` is the single source of truth for the CSS, and this is
 * the only place that writes it after the inline script in `Base.astro`.
 */
function apply() {
  document.documentElement.classList.toggle('dark', resolved() === 'dark');
  for (const notify of watchers) notify();
}

export function set(theme: Theme) {
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // A reader who cannot store a preference still gets to change the view.
  }
  apply();
}

/**
 * Registered once at module scope rather than per subscriber.
 *
 * A `storage` event fires only for *other* tabs, and `change` on the media query
 * only when the OS switches, so neither can be triggered by `set` above — there
 * is no loop here. The guard is for the island's server render, where this module
 * is evaluated and there is no `document`.
 */
if (typeof document !== 'undefined') {
  addEventListener('storage', apply);
  os().addEventListener('change', apply);
}

export function subscribe(notify: () => void) {
  watchers.add(notify);
  return () => {
    watchers.delete(notify);
  };
}
