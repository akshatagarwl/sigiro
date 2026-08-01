'use client';

import { useMachine } from '@xstate/react';
import { useEffect } from 'react';
import { createMachine, types } from 'xstate';

/**
 * Which key is down — and the honest third answer, which is that nobody here
 * knows yet.
 *
 * `unknown` is not a spinner state and it is not "light". The theme class is
 * already on `<html>` before first paint, put there by the blocking script in
 * the root layout; what cannot see it is this component's first render, which
 * happens on the server. So the truth on that render is genuinely "unresolved",
 * and it stays a state rather than collapsing into a boolean because the two
 * things that read it want different answers: the cap draws at zero opacity
 * (its cell is reserved from the start, so nothing moves when it appears), and
 * *neither* button is `aria-pressed`. A boolean would have to guess, and a
 * wrong guess is exactly the flash of the wrong theme the script exists to
 * prevent.
 *
 * Two events rather than one, because they are two different sentences.
 * `OBSERVED` reads what the pre-paint script already decided and is only
 * accepted in `unknown` — resolving the unknown is a thing that happens once,
 * and the machine is where that is stated. `CHOSE` is a click, accepted
 * anywhere, and is the only one that writes anything down.
 *
 * The writes themselves — the class on `<html>`, the localStorage key — stay in
 * the handler rather than becoming entry actions. As an entry action on
 * `dark`/`light` they would also fire for `OBSERVED`, which would persist a
 * preference the reader never expressed and quietly freeze whatever
 * `prefers-color-scheme` happened to say on their first visit.
 */
function pick({ event }: { event: { dark: boolean } }) {
  return { target: event.dark ? ('dark' as const) : ('light' as const) };
}

const theme = createMachine({
  schemas: {
    events: { OBSERVED: types<{ dark: boolean }>(), CHOSE: types<{ dark: boolean }>() },
  },
  initial: 'unknown',
  states: {
    unknown: { on: { OBSERVED: pick, CHOSE: pick } },
    light: { on: { CHOSE: pick } },
    dark: { on: { CHOSE: pick } },
  },
});

/**
 * Light and dark, as two keys in a well with one cap sliding between them.
 *
 * The icons are drawn inline on the Phosphor grid (viewBox 256, strokeWidth 16 —
 * 1.5px at this render size) rather than imported, because an icon package would
 * be a whole dependency for two glyphs.
 */
export function ThemeToggle() {
  const [state, send] = useMachine(theme);
  const known = !state.matches('unknown');
  const dark = state.matches('dark');

  useEffect(() => {
    send({ type: 'OBSERVED', dark: document.documentElement.classList.contains('dark') });
  }, [send]);

  function set(next: boolean) {
    document.documentElement.classList.toggle('dark', next);
    try {
      localStorage.setItem('sigiro-theme', next ? 'dark' : 'light');
    } catch {
      // A blocked localStorage costs the preference, not the toggle.
    }
    send({ type: 'CHOSE', dark: next });
  }

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
        aria-pressed={state.matches('light')}
        className="relative z-10 grid h-5 w-6 place-items-center rounded-[5px]"
        style={{ color: state.matches('light') ? 'var(--ink)' : 'var(--ink-3)' }}
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
