import { DOCS } from '@/lib/docs';
import { SITE } from '@/lib/site';

/**
 * Every doc as markdown in one request \u2014 the source, not scraped HTML, and not
 * one fetch per page. This is the surface an agent evaluating sigiro wants.
 */
export const dynamic = 'force-static';

export function GET() {
  const sections = DOCS.map((doc) => `<!-- ${SITE.url}${doc.url} -->\n\n${doc.markdown}`);

  const body = `# ${SITE.name} \u2014 full documentation

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
