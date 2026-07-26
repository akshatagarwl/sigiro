'use client';

import { CheckIcon, CopyIcon } from 'lucide-react';
import { useRef, useState } from 'react';
import { Button } from '@/components/ui/button';

/**
 * Extracted from @kibo-ui/snippet, which shipped a whole tabbed-snippet family for
 * one button — and defaulted it to `opacity-0 group-hover:opacity-100`, invisible
 * on a touch screen. This is the same clipboard behaviour, always visible.
 *
 * `value` is optional because a docs fence has no plain-text form to pass: by the
 * time MDX renders it, the code is a tree of Shiki-coloured spans. In that case
 * the text comes off the rendered <pre> instead, which is also the only copy that
 * is guaranteed to match what the reader sees.
 */
export function CopyButton({ value, label }: { value?: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);

  return (
    <Button
      ref={ref}
      size="icon-xs"
      variant="ghost"
      aria-label={label ? `Copy ${label}` : 'Copy code'}
      onClick={() => {
        const text =
          value ?? ref.current?.closest('figure')?.querySelector('pre')?.textContent ?? '';
        if (!text || !navigator.clipboard?.writeText) return;
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 2000);
        });
      }}
    >
      {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
    </Button>
  );
}
