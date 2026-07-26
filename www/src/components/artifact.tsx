import { highlight, type Lang } from '@/lib/highlight';

/**
 * Where polylane shows a screenshot of its UI, we show the thing sigiro actually
 * returns — this product has no screen. Shiki colours the JSON, SQL and shell
 * server-side, so no syntax-highlighting JavaScript is shipped to the browser.
 */
export async function Artifact({
  title,
  meta,
  code,
  lang,
}: {
  title: string;
  meta?: string;
  code: string;
  lang: Lang;
}) {
  const html = await highlight(code, lang);

  return (
    <figure className="mt-6 overflow-hidden rounded-xl border border-line bg-card">
      <figcaption className="flex items-center justify-between gap-4 border-b border-line bg-surface px-3.5 py-2 font-mono text-[0.6875rem] text-muted-ink">
        <span className="font-medium text-strong">{title}</span>
        {meta ? <span className="truncate">{meta}</span> : null}
      </figcaption>
      <div
        className="overflow-x-auto bg-code-bg p-3.5"
        // Shiki output, generated from literals in this repo.
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </figure>
  );
}
