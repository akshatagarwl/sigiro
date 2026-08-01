import { CopySource } from '@/components/site/copy-source';

/**
 * The one code-block shell for the whole site: the landing page artifacts and
 * every fence in the docs render through this, so they are the same object.
 *
 * The shape is the design language's: a `mat-panel` frame at radius 14 with
 * `p-[5px]`, holding a caption row and a `mat-well` at radius 9 — the code is cut
 * into the card, not bordered on it. 14 = 9 + 5, which is the whole reason those
 * two numbers are those two numbers.
 *
 * The caption is `.meta`: mono, 10.5px, the instrument voice. It is metadata about
 * the block, never prose.
 */
export function CodeBlock({
  lang,
  title,
  meta,
  value,
  children,
}: {
  /** Language, shown when there is nothing more specific to say. */
  lang?: string;
  title?: string;
  meta?: string;
  /** The plain text the copy cap puts on the clipboard. Omitted only where there
   *  genuinely is none to have. */
  value?: string;
  /** Pre-highlighted markup — Shiki output, generated server-side. */
  children: React.ReactNode;
}) {
  const caption = [title, meta].filter(Boolean).join(' · ') || lang;

  return (
    <figure className="not-prose mat-panel my-6 rounded-[14px] p-[5px]">
      <figcaption className="flex h-8 items-center gap-2.5 pr-[3px] pl-2.5">
        <span className="meta min-w-0 flex-1 truncate text-ink-3">{caption}</span>
        {value ? <CopySource code={value} label={title ?? lang} /> : null}
      </figcaption>
      {/* The type is set here and nowhere else: a fence in the docs sits inside a
          `.prose` region and one on the landing page does not, so leaving the
          size to inherit is what made the two diverge. */}
      <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[1.7]">
        {children}
      </div>
    </figure>
  );
}
