import { SnippetCopyButton } from '@/components/kibo-ui/snippet';

/**
 * The one code-block shell for the whole site: the landing page artifacts and
 * every fence in the docs render through this, so they are the same object.
 *
 * The copy button is `@kibo-ui/snippet`'s — it owns the clipboard call and the
 * copied/idle state. `always` overrides the registry default of
 * `opacity-0 group-hover:opacity-100`, which is invisible to the eye until you
 * mouse over it and does not exist at all on a touch screen.
 */
export function CodeBlock({
  title,
  meta,
  value,
  children,
}: {
  title?: string;
  meta?: string;
  /** The plain text the copy button puts on the clipboard. */
  value: string;
  /** Pre-highlighted markup — Shiki output, generated server-side. */
  children: React.ReactNode;
}) {
  return (
    <figure className="not-prose group my-6 overflow-hidden rounded-xl border border-line bg-code-bg">
      <figcaption className="flex items-center justify-between gap-3 border-b border-line bg-surface px-3.5 py-1.5 font-mono text-[0.6875rem] text-muted-ink">
        <span className="truncate">
          {title ? <span className="font-medium text-strong">{title}</span> : null}
          {title && meta ? ' · ' : null}
          {meta}
        </span>
        <SnippetCopyButton
          value={value}
          aria-label={title ? `Copy ${title}` : 'Copy code'}
          className="-mr-1 opacity-100"
        />
      </figcaption>
      <div className="overflow-x-auto p-3.5">{children}</div>
    </figure>
  );
}
