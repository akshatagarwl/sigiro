'use client';

import { useMachine } from '@xstate/react';
import { createMachine } from 'xstate';

/**
 * The copy affordance a code fence gets: a 22px cap in the caption row, labelled
 * `copy` in mono, that swaps for a moss check on success.
 *
 * Both faces sit in the same grid cell and only opacity moves, so the cap keeps
 * its width when the state changes. The check is drawn inline on the Phosphor
 * grid rather than imported — one glyph is not worth a dependency.
 *
 * The check is a state, not a flag with a timer beside it. `copied` owns its own
 * 2s exit, and copying again while the check is up re-enters `copied`, which
 * cancels and restarts that exit — the same thing the hand-rolled version did by
 * clearing a ref'd timeout, minus the ref, the unmount cleanup and the chance of
 * forgetting either.
 */

/** How long the check stays up. */
const HELD_MS = 2000;

const clipboard = createMachine({
  initial: 'idle',
  states: {
    idle: {
      on: { COPIED: { target: 'copied' } },
    },
    copied: {
      // Re-entering restarts the delayed exit below, so a second copy holds the
      // check for a fresh 2s rather than inheriting the first one's remainder.
      on: { COPIED: { target: 'copied', reenter: true } },
      after: { [HELD_MS]: { target: 'idle' } },
    },
  },
});

export function CopySource({ code, label }: { code: string; label?: string }) {
  const [state, send] = useMachine(clipboard);
  const copied = state.matches('copied');

  // The write itself stays here rather than becoming an invoked actor: it is one
  // await with one failure mode, and the failure is "nothing happens", which is
  // the machine staying where it already is.
  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      return;
    }
    send({ type: 'COPIED' });
  }

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
