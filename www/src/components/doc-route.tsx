import type { OpenAPIPageProps } from 'fumadocs-openapi/ui';
import defaultMdxComponents from 'fumadocs-ui/mdx';
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/page';
import { OpenAPIPage } from '@/components/openapi-page';
import { openapi } from '@/lib/openapi';
import { SITE } from '@/lib/site';
import type { source } from '@/lib/source';

type DocPage = NonNullable<ReturnType<typeof source.getPage>>;

/**
 * One body for every docs route, whether it arrived from a static route file or
 * the catch-all. The four hand-written docs get static routes purely so their
 * description reaches `<head>`: under vinext an async `generateMetadata` streams
 * its tags into `<body>`, where a crawler that does not run JavaScript never sees
 * them. The generated API pages stay on the catch-all, where that is the lesser
 * evil against writing a route file per operation.
 */
export async function DocRoute({ page }: { page: DocPage }) {
  const MDX = page.data.body;
  // Generated reference pages declare `_openapi.preload`; the schema has to be
  // resolved on the server and handed to the client component.
  const preloaded = page.data._openapi ? await openapi.preloadOpenAPIPage(page) : undefined;

  const schema = {
    '@context': 'https://schema.org',
    '@type': 'TechArticle',
    headline: page.data.title,
    description: page.data.description,
    url: `${SITE.url}${page.url}`,
    isPartOf: { '@id': `${SITE.url}/#website` },
    publisher: { '@id': `${SITE.url}/#organization` },
  };

  return (
    <DocsPage toc={page.data.toc} full={page.data.full}>
      <DocsTitle>{page.data.title}</DocsTitle>
      <DocsDescription>{page.data.description}</DocsDescription>
      <DocsBody>
        <MDX
          components={{
            ...defaultMdxComponents,
            // The MDX supplies `document` and `operations`; `preloaded` carries
            // the resolved schema. Merged and cast once, because the prop type is
            // a union and neither half satisfies it alone.
            APIPage: (props: object) => (
              <OpenAPIPage {...({ ...props, ...preloaded } as OpenAPIPageProps)} />
            ),
          }}
        />
      </DocsBody>
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(schema) }}
      />
    </DocsPage>
  );
}
