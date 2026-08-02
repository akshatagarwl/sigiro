'use client';

import { useTheme } from 'next-themes';
import { useSyncExternalStore } from 'react';

/** Nothing to subscribe to: this value changes once, when React hydrates. */
const never = () => () => {};

/**
 * False through the server render and the hydrating render, true afterwards.
 *
 * `next-themes` reads storage in a lazy `useState` initializer, so by its own
 * first client render it already knows the theme — while the server render
 * could not have. Deriving "do we know yet" from `resolvedTheme` therefore
 * answers *yes* one render too early, and the server's `aria-pressed="false"`
 * meets a client that wants `true`. React logged exactly that and threw the
 * hydrated tree away.
 *
 * `useSyncExternalStore` is the fix rather than a `useState` in a `useEffect`,
 * because React feeds the server snapshot to the hydrating render as well —
 * which is the property being relied on here, not a side effect of one.
 */
function useHydrated() {
  return useSyncExternalStore(
    never,
    () => true,
    () => false,
  );
}

/**
 * Light and dark, as two keys in a well with one cap sliding between them.
 *
 * The icons are drawn inline on the Phosphor grid (viewBox 256, strokeWidth 16 —
 * 1.5px at this render size) rather than imported, because an icon package would
 * be a whole dependency for two glyphs.
 *
 * The state was a three-state XState machine here: `unknown` / `light` / `dark`,
 * with an `OBSERVED` event that read the class the pre-paint script had written
 * and a `CHOSE` event that wrote the preference. It is now `useTheme`, which
 * owns the same three answers and also hears the two changes the machine could
 * not: the operating system switching theme while the page is open, and another
 * tab making the choice. See the note in `app/layout.tsx`.
 *
 * `resolvedTheme` is `undefined` until the provider has mounted, and that is the
 * old `unknown` state exactly — not a spinner and not "light". Two things read
 * it and want different answers: the sliding cap draws at zero opacity, its cell
 * reserved from the start so nothing moves when it appears, and *neither* button
 * is `aria-pressed`. Guessing would show the wrong key for a frame, which is the
 * same class of bug as a flash of the wrong theme.
 */
export function ThemeToggle() {
  const { resolvedTheme, setTheme } = useTheme();
  const known = useHydrated() && resolvedTheme !== undefined;
  const dark = known && resolvedTheme === 'dark';

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
