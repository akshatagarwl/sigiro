import { type Doc, renderDoc } from '@/lib/docs';
import { SITE } from '@/lib/site';

/**
 * Shared body for every docs route. Each doc gets its own route file with a
 * static `metadata` export rather than one catch-all with `generateMetadata`:
 * under vinext, async generateMetadata streams its <title>/<meta> into <body>
 * instead of <head>, which reads as a missing meta description to anything that
 * does not run JavaScript.
 *
 * The prose classes map typography onto the same tokens the landing page uses, so
 * a heading, a rule and a code block look identical on both sides of the site.
 */
const PROSE = [
  'prose max-w-none',
  'prose-headings:font-semibold prose-headings:tracking-[-0.025em] prose-headings:text-strong',
  'prose-h1:text-[1.75rem] prose-h1:leading-[1.15] prose-h2:text-[1.375rem] prose-h3:text-[1.0625rem]',
  'prose-p:text-body prose-li:text-body prose-strong:text-strong prose-strong:font-semibold',
  'prose-a:text-strong prose-a:decoration-line-strong prose-a:underline-offset-2',
  'prose-hr:border-line prose-blockquote:border-l-line prose-blockquote:text-muted-ink',
  // inline code and fenced blocks are styled in globals.css — `prose-code:`
  // matches code inside `pre` as well, and its specificity wins there.
  'prose-pre:rounded-xl prose-pre:border prose-pre:border-line',
  'prose-table:text-[0.875rem] prose-th:text-strong prose-th:font-medium',
].join(' ');

export async function DocPage({ doc }: { doc: Doc }) {
  const html = await renderDoc(doc);

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
      {/* Markdown authored in this repo, not user input. */}
      <div dangerouslySetInnerHTML={{ __html: html }} />
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(schema) }}
      />
    </article>
  );
}
