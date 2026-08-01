'use client';

import { useEffect, useRef, useState } from 'react';

/**
 * The copy affordance a code fence gets: a 22px cap in the caption row, labelled
 * `copy` in mono, that swaps for a moss check on success.
 *
 * Both faces sit in the same grid cell and only opacity moves, so the cap keeps
 * its width when the state changes. The check is drawn inline on the Phosphor
 * grid rather than imported — one glyph is not worth a dependency.
 */
export function CopySource({ code, label }: { code: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      return;
    }
    setCopied(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 2000);
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
