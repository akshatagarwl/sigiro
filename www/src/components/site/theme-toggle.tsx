import { useSyncExternalStore } from 'react';
import { resolved, set as setTheme, subscribe } from '@/components/site/theme';

/**
 * Light and dark, as two keys in a well with one cap sliding between them.
 *
 * The icons are drawn inline on the Phosphor grid (viewBox 256, strokeWidth 16 —
 * 1.5px at this render size) rather than imported, because an icon package would
 * be a whole dependency for two glyphs.
 *
 * The state has been three things: a three-state XState machine, then
 * `next-themes`, and now `components/site/theme.ts` — see the note there for why
 * next-themes could not follow us to Astro. All three answer the same three
 * questions, and the store keeps the two behaviours the machine never had: the
 * operating system switching theme while the page is open, and another tab making
 * the choice.
 *
 * `null` is the third answer and it is load-bearing — not a spinner and not
 * "light", but *we do not know yet*. `useSyncExternalStore` produces it exactly
 * where it is true: the server snapshot is a constant `null`, and React feeds
 * that same snapshot to the hydrating render, so the markup Astro built and the
 * markup React first renders agree by construction rather than by luck. Reading
 * storage during render instead would make the two disagree, which is what makes
 * React throw the hydrated tree away.
 *
 * Two things read it and want different answers: the sliding cap draws at zero
 * opacity, its cell reserved from the start so nothing moves when it appears, and
 * *neither* button is `aria-pressed`. Guessing would show the wrong key for a
 * frame, which is the same class of bug as a flash of the wrong theme.
 */
export function ThemeToggle() {
  const theme = useSyncExternalStore(subscribe, resolved, () => null);
  const known = theme !== null;
  const dark = theme === 'dark';

  // Writes a concrete choice rather than "system", which is what clicking one of
  // two keys means. A reader who wants to follow the OS again clears the site's
  // storage; there is no third key, and adding one would be a new control rather
  // than a refactor.
  const set = (next: boolean) => setTheme(next ? 'dark' : 'light');

  return (
    <div
      className="mat-well relative flex h-[26px] w-[54px] items-center rounded-[8px] p-[3px]"
      role="group"
      aria-label="Theme"
    >
      <span
        aria-hidden
        className="mat-cap absolute top-[3px] left-[3px] h-5 w-6 rounded-[5px] transition-transform duration-[220ms] ease-[cubic-bezier(0.23,1,0.32,1)]"
        style={{
          transform: dark ? 'translateX(24px)' : 'translateX(0px)',
          opacity: known ? 1 : 0,
        }}
      />
      <button
        type="button"
        onClick={() => set(false)}
        aria-label="Light"
        aria-pressed={known && !dark}
        className="relative z-10 grid h-5 w-6 place-items-center rounded-[5px]"
        style={{ color: known && !dark ? 'var(--ink)' : 'var(--ink-3)' }}
      >
        <Sun />
      </button>
      <button
        type="button"
        onClick={() => set(true)}
        aria-label="Dark"
        aria-pressed={dark}
        className="relative z-10 grid h-5 w-6 place-items-center rounded-[5px]"
        style={{ color: dark ? 'var(--ink)' : 'var(--ink-3)' }}
      >
        <Moon />
      </button>
    </div>
  );
}

function Sun() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 256 256"
      fill="none"
      stroke="currentColor"
      strokeWidth="16"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <circle cx="128" cy="128" r="52" />
      <path d="M128 28V16M128 240v-12M60 60l-8-8M204 204l-8-8M28 128H16M240 128h-12M60 196l-8 8M204 52l-8 8" />
    </svg>
  );
}

function Moon() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 256 256"
      fill="none"
      stroke="currentColor"
      strokeWidth="16"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M216 152A96 96 0 0 1 104 40a96 96 0 1 0 112 112Z" />
    </svg>
  );
}
