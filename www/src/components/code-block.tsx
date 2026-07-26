import { CopyButton } from '@/components/copy-button';

/**
 * The one code-block shell for the whole site: the landing page artifacts and
 * every fence in the docs render through this, so they are the same object.
 *
 * The copy button is always visible — see components/copy-button.tsx for why the
 * registry's version was replaced.
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
  /** The plain text the copy button puts on the clipboard. Omit in MDX — the
   *  button reads the rendered <pre> instead. */
  value?: string;
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
        <CopyButton value={value} label={title ?? lang} />
      </figcaption>
      {/* The type is set here and nowhere else: a fence in the docs sits inside a
          `.prose` region and one on the landing page does not, so leaving the
          size to inherit is what made the two diverge. */}
      <div className="overflow-x-auto p-3.5 font-mono text-[0.8125rem] leading-[1.7]">
        {children}
      </div>
    </figure>
  );
}
