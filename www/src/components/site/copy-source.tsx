'use client';

import { useCopyButton } from 'fumadocs-ui/utils/use-copy-button';

/**
 * The copy affordance a code fence gets: a 22px cap in the caption row, labelled
 * `copy` in mono, that swaps for a moss check on success.
 *
 * Both faces sit in the same grid cell and only opacity moves, so the cap keeps
 * its width when the state changes. The check is drawn inline on the Phosphor
 * grid rather than imported — one glyph is not worth a dependency.
 *
 * The held check used to be an XState machine here: an `idle`/`copied` pair with
 * a delayed exit that re-entering restarted. It was correct and it was a
 * re-implementation. `useCopyButton` ships with fumadocs, which this site
 * already depends on for the whole docs surface, and it does the same three
 * things — hold the flag, restart the timer when you copy again, clear it on
 * unmount. A machine is the right shape for the scroll sequence in terminal.tsx,
 * where states mean something; here it was two states and a timeout.
 *
 * Its hold is 1500ms where ours was 2000ms. Not worth a fork.
 */
export function CopySource({ code, label }: { code: string; label?: string }) {
  /**
   * The rejection is deliberately not swallowed.
   *
   * `useCopyButton` raises its flag in a `.then`, so a rejected write skips it
   * and the cap correctly stays on `copy` — a check that appeared when nothing
   * reached the clipboard would be a lie. The old version returned early and
   * said nothing at all, which is the silent-failure this repo's rules forbid.
   * Logging and rethrowing keeps both properties: no false check, and a reason
   * in the console when a reader reports that copy does nothing.
   */
  const [copied, copy] = useCopyButton(() =>
    navigator.clipboard.writeText(code).catch((error: unknown) => {
      console.error('sigiro: could not write to the clipboard', error);
      throw error;
    }),
  );

  return (
    <button
      type="button"
      onClick={copy}
      aria-label={label ? `Copy ${label}` : 'Copy code'}
      className="mat-cap press grid h-[22px] shrink-0 place-items-center rounded-[6px] px-2 text-[10.5px] font-medium text-ink-2 hover:text-ink"
    >
      <span
        className="col-start-1 row-start-1 transition-opacity duration-100"
        style={{ opacity: copied ? 0 : 1 }}
      >
        copy
      </span>
      <span
        aria-hidden
        className="col-start-1 row-start-1 inline-flex items-center text-moss transition-opacity duration-100"
        style={{ opacity: copied ? 1 : 0 }}
      >
        <svg
          width="11"
          height="11"
          viewBox="0 0 256 256"
          fill="none"
          stroke="currentColor"
          strokeWidth="24"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden
        >
          <path d="M216 72 104 184l-56-56" />
        </svg>
      </span>
      <span className="sr-only" role="status">
        {copied ? 'Copied' : ''}
      </span>
    </button>
  );
}
