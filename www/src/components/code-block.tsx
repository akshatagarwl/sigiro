import { SnippetCopyButton } from '@/components/kibo-ui/snippet';

/**
 * The one code-block shell for the whole site: the landing page artifacts and
 * every fence in the docs render through this, so they are the same object.
 *
 * The copy button is `@kibo-ui/snippet`'s — it owns the clipboard call and the
 * copied/idle state. `opacity-100` overrides the registry default of
 * `opacity-0 group-hover:opacity-100`, which is invisible to the eye until you
 * mouse over it and does not exist at all on a touch screen.
 */
export function CodeBlock({
  lang,
  title,
  meta,
  value,
  children,
}: {
  /** Language label, shown as the leading tag. */
  lang?: string;
  title?: string;
  meta?: string;
  /** The plain text the copy button puts on the clipboard. */
  value: string;
  /** Pre-highlighted markup — Shiki output, generated server-side. */
  children: React.ReactNode;
}) {
  return (
    <figure className="not-prose group my-6 overflow-hidden rounded-xl border border-line bg-code-bg">
      <figcaption className="flex items-center gap-2.5 border-b border-line bg-surface py-1.5 pr-1.5 pl-3.5 font-mono text-[0.6875rem] text-muted-ink">
        {lang ? (
          <span className="shrink-0 rounded border border-line bg-card px-1.5 py-0.5 text-[0.625rem] tracking-[0.08em] text-faint uppercase">
            {lang}
          </span>
        ) : null}
        <span className="min-w-0 flex-1 truncate">
          {title ? <span className="font-medium text-strong">{title}</span> : null}
          {title && meta ? ' · ' : null}
          {meta}
        </span>
        <SnippetCopyButton
          value={value}
          aria-label={title ? `Copy ${title}` : `Copy ${lang ?? 'code'}`}
          className="shrink-0 opacity-100"
        />
      </figcaption>
      <div className="overflow-x-auto p-3.5">{children}</div>
    </figure>
  );
}
