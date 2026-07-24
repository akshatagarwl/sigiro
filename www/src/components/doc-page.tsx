import type { Doc } from '@/lib/docs';
import { SITE } from '@/lib/site';

/**
 * Shared body for every docs route. Each doc gets its own route file with a
 * static `metadata` export rather than one catch-all with `generateMetadata`:
 * under vinext, async generateMetadata streams its <title>/<meta> into <body>
 * instead of <head>, which reads as a missing meta description to anything that
 * does not run JavaScript.
 */
export function DocPage({ doc }: { doc: Doc }) {
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
    <article className="prose prose-neutral max-w-none dark:prose-invert prose-headings:font-semibold prose-headings:tracking-tight prose-pre:border prose-pre:border-neutral-200 prose-pre:bg-neutral-50 prose-pre:text-neutral-900 dark:prose-pre:border-neutral-800 dark:prose-pre:bg-neutral-900 dark:prose-pre:text-neutral-100">
      {/* Markdown is authored in this repo, not user input. */}
      <div dangerouslySetInnerHTML={{ __html: doc.html }} />
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(schema) }}
      />
    </article>
  );
}
