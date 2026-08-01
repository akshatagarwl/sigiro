'use client';

import { useEffect, useState } from 'react';

/**
 * Light and dark, as two keys in a well with one cap sliding between them.
 *
 * The icons are drawn inline on the Phosphor grid (viewBox 256, strokeWidth 16 —
 * 1.5px at this render size) rather than imported, because an icon package would
 * be a whole dependency for two glyphs.
 *
 * `dark === null` is the first render: the class is on <html> before paint (see
 * the theme script in the root layout) but React cannot read it during SSR, so
 * the cap is drawn at zero opacity until the effect resolves which side it is on.
 * Its cell is reserved from the start, so nothing moves.
 */
export function ThemeToggle() {
  const [dark, setDark] = useState<boolean | null>(null);

  useEffect(() => {
    setDark(document.documentElement.classList.contains('dark'));
  }, []);

  function set(next: boolean) {
    document.documentElement.classList.toggle('dark', next);
    try {
      localStorage.setItem('sigiro-theme', next ? 'dark' : 'light');
    } catch {
      // A blocked localStorage costs the preference, not the toggle.
    }
    setDark(next);
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
          opacity: dark === null ? 0 : 1,
        }}
      />
      <button
        type="button"
        onClick={() => set(false)}
        aria-label="Light"
        aria-pressed={dark === false}
        className="relative z-10 grid h-5 w-6 place-items-center rounded-[5px]"
        style={{ color: dark === false ? 'var(--ink)' : 'var(--ink-3)' }}
      >
        <Sun />
      </button>
      <button
        type="button"
        onClick={() => set(true)}
        aria-label="Dark"
        aria-pressed={dark === true}
        className="relative z-10 grid h-5 w-6 place-items-center rounded-[5px]"
        style={{ color: dark === true ? 'var(--ink)' : 'var(--ink-3)' }}
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
