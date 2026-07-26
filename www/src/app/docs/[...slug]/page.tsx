import type { OpenAPIPageProps } from 'fumadocs-openapi/ui';
import defaultMdxComponents from 'fumadocs-ui/mdx';
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/page';
import { notFound } from 'next/navigation';
import { OpenAPIPage } from '@/components/openapi-page';
import { openapi } from '@/lib/openapi';
import { SITE } from '@/lib/site';
import { source } from '@/lib/source';

export const dynamic = 'force-static';

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata({ params }: { params: Promise<{ slug?: string[] }> }) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  return {
    title: page.data.title,
    description: page.data.description,
    alternates: { canonical: page.url },
  };
}

export default async function Page({ params }: { params: Promise<{ slug?: string[] }> }) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();

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
        {/* APIPage is what renders the generated reference and its playground. */}
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
