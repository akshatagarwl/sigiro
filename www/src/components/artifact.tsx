import { CodeBlock } from '@/components/code-block';
import { highlight, type Lang } from '@/lib/highlight';

/**
 * Where polylane shows a screenshot of its UI, we show the thing sigiro actually
 * returns — this product has no screen. Shiki colours it server-side, so no
 * highlighting JavaScript reaches the browser, and the shell is the same
 * CodeBlock every fence in the docs uses.
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
    <CodeBlock lang={lang} title={title} meta={meta} value={code}>
      {/* Shiki output, generated from literals in this repo. */}
      <div dangerouslySetInnerHTML={{ __html: html }} />
    </CodeBlock>
  );
}
