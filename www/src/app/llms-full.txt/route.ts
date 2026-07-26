import { SITE } from '@/lib/site';
import { source } from '@/lib/source';

/**
 * Every doc as markdown in one request — the source, not scraped HTML, and not
 * one fetch per page. `getText('processed')` works because source.config.ts
 * enables `includeProcessedMarkdown`.
 */
export const dynamic = 'force-static';

export async function GET() {
  const sections = await Promise.all(
    source.getPages().map(async (page) => {
      const markdown = await page.data.getText('processed');
      return `<!-- ${SITE.url}${page.url} -->\n\n${markdown}`;
    }),
  );

  const body = `# ${SITE.name} — full documentation

> ${SITE.description}

${sections.join('\n\n---\n\n')}
`;

  return new Response(body, {
    headers: {
      'content-type': 'text/plain; charset=utf-8',
      'cache-control': 'public, max-age=3600',
    },
  });
}
