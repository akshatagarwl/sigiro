import { source } from '@/app/source';
import { SITE } from '@/lib/site';

/**
 * Every doc as markdown in one request. This is the surface an agent evaluating
 * sigiro actually wants: the source, not scraped HTML, and not one fetch per
 * page. `getText('processed')` works because source.config.ts enables
 * `includeProcessedMarkdown`.
 */
export const dynamic = 'force-static';

export async function GET() {
  const pages = source.getPages();

  const sections = await Promise.all(
    pages.map(async (page) => {
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
