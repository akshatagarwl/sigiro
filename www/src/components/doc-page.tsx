import rehypeShikiFromHighlighter from '@shikijs/rehype/core';
import type { Element } from 'hast';
import { toString } from 'hast-util-to-string';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { CodeBlock } from '@/components/code-block';
import type { Doc } from '@/lib/docs';
import { COLOR_REPLACEMENTS, getHighlighter } from '@/lib/highlight';
import { SITE } from '@/lib/site';

/**
 * Shared body for every docs route. Each doc gets its own route file with a
 * static `metadata` export rather than one catch-all with `generateMetadata`:
 * under vinext, async generateMetadata streams its <title>/<meta> into <body>
 * instead of <head>, which reads as a missing meta description to anything that
 * does not run JavaScript.
 *
 * Fences go through @shikijs/rehype with the same Workers-safe highlighter the
 * landing page uses, and react-markdown maps `pre` onto the same CodeBlock — so a
 * bash block in the quickstart is the same component, copy button and all, as the
 * one in the hero.
 */
const PROSE = [
  'prose max-w-none',
  'prose-headings:font-semibold prose-headings:tracking-[-0.025em] prose-headings:text-strong',
  'prose-h1:text-[1.75rem] prose-h1:leading-[1.15] prose-h2:text-[1.375rem] prose-h3:text-[1.0625rem]',
  'prose-p:text-body prose-li:text-body prose-strong:text-strong prose-strong:font-semibold',
  'prose-a:text-strong prose-a:decoration-line-strong prose-a:underline-offset-2',
  'prose-hr:border-line prose-blockquote:border-l-line prose-blockquote:text-muted-ink',
  'prose-table:text-[0.875rem] prose-th:text-strong prose-th:font-medium',
].join(' ');

export async function DocPage({ doc }: { doc: Doc }) {
  const highlighter = await getHighlighter();

  const schema = {
    '@context': 'https://schema.org',
    '@type': 'TechArticle',
    headline: doc.title,
    description: doc.description,
    url: `${SITE.url}${doc.url}`,
    isPartOf: { '@id': `${SITE.url}/#website` },
    publisher: { '@id': `${SITE.url}/#organization` },
  };

  return (
    <article className={PROSE}>
      <Markdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[
          [
            rehypeShikiFromHighlighter,
            highlighter,
            {
              theme: 'github-light-default',
              colorReplacements: COLOR_REPLACEMENTS,
              // A fence whose language we have no grammar for stays plain text
              // rather than failing the whole render.
              fallbackLanguage: 'text',
              // Off by default, and without it the language does not survive the
              // transform — there is nothing left on the node to label the block.
              addLanguageClass: true,
            },
          ],
        ]}
        components={{
          pre({ node, children, ...props }) {
            // `node` is the hast <pre>; its text is what the clipboard needs, and
            // its <code> child carries the `language-*` class rehype-shiki adds.
            const value = node ? toString(node as Element) : '';
            const code = node?.children?.find(
              (c): c is Element => c.type === 'element' && c.tagName === 'code',
            );
            // rehype-shiki writes the raw attribute name `class`, not hast's
            // normalised `className` — read both, and accept either the string or
            // the array form.
            const props_ = (code?.properties ?? {}) as Record<string, unknown>;
            const raw = props_.class ?? props_.className;
            const classes = (Array.isArray(raw) ? raw : String(raw ?? '').split(/\s+/)).filter(
              (c): c is string => typeof c === 'string',
            );
            const lang = classes
              .find((c) => c.startsWith('language-'))
              ?.slice('language-'.length);
            // Wrap the original <pre>, never replace it — dropping it collapses
            // the whitespace and loses Shiki's class and inline theme colours.
            return (
              <CodeBlock lang={lang === 'text' ? undefined : lang} value={value}>
                <pre {...props}>{children}</pre>
              </CodeBlock>
            );
          },
        }}
      >
        {doc.markdown}
      </Markdown>
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(schema) }}
      />
    </article>
  );
}
