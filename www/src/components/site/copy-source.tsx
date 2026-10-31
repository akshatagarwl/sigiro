"use client";

import { useCallback, useEffect, useRef, useState } from "react";

/** How long the check stays up. */
const HELD_MS = 1500;

/**
 * Copy, then hold a check; copying again restarts the hold; the timer is cleared
 * on unmount.
 *
 * This was `useCopyButton` from fumadocs, which was the right answer while
 * fumadocs was a dependency. The docs moved to Blume on their own origin and
 * fumadocs is gone, and keeping an entire docs framework installed for one
 * fifteen-line hook is not a trade worth making. Nothing else in the tree ships
 * it — usehooks-ts's `useCopyToClipboard` has no timed reset, which is the only
 * part with any subtlety.
 */
function useCopyButton(onCopy: () => Promise<void>): [boolean, () => void] {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  // A ref, so the callback identity below stays stable while `code` changes.
  // Updated in an effect, not during render: writing a ref while rendering made
  // the React Compiler skip this whole component. A click always lands after
  // the commit, so the ref is current by then.
  const latest = useRef(onCopy);
  useEffect(() => {
    latest.current = onCopy;
  });

  useEffect(() => () => window.clearTimeout(timer.current ?? undefined), []);

  const click = useCallback(() => {
    if (timer.current) window.clearTimeout(timer.current);
    // No `catch`: a rejected write must not raise the check, and this repo's
    // rules say a failure is reported rather than swallowed.
    latest.current().then(() => {
      setCopied(true);
      timer.current = window.setTimeout(() => setCopied(false), HELD_MS);
    });
  }, []);

  return [copied, click];
}

/**
 * The copy affordance a code fence gets: a 24px cap in the caption row, labelled
 * `copy` in mono, that swaps for a moss check on success.
 *
 * Both faces sit in the same grid cell and only opacity moves, so the cap keeps
 * its width when the state changes. The check is drawn inline on the Phosphor
 * grid rather than imported — one glyph is not worth a dependency.
 *
 * The held check has been three things. First an XState machine — an
 * `idle`/`copied` pair with a delayed exit that re-entering restarted. Then
 * fumadocs' `useCopyButton`, once it turned out an installed dependency already
 * shipped exactly that. Now the small hook above, because the docs left for
 * Blume and fumadocs went with them.
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
  const [copied, copy] = useCopyButton(async () => {
    try {
      await navigator.clipboard.writeText(code);
    } catch (cause) {
      const error = cause instanceof Error ? cause : new Error(String(cause));
      console.error("sigiro: could not write to the clipboard", error);
      throw error;
    }
  });

  return (
    <button
      type="button"
      onClick={copy}
      aria-label={label ? `Copy ${label}` : "Copy code"}
      className="mat-cap press text-ink-2 hover:text-ink grid h-6 shrink-0 place-items-center rounded-[6px] px-2 text-[10.5px] font-medium"
    >
      <span
        className="col-start-1 row-start-1 transition-opacity duration-100"
        style={{ opacity: copied ? 0 : 1 }}
      >
        copy
      </span>
      <span
        aria-hidden
        className="text-moss col-start-1 row-start-1 inline-flex items-center transition-opacity duration-100"
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
        {copied ? "Copied" : ""}
      </span>
    </button>
  );
}
