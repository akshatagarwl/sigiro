import type { OpenAPIPageProps } from 'fumadocs-openapi/ui';
import defaultMdxComponents from 'fumadocs-ui/mdx';
import { isValidElement, type ComponentProps, type ReactNode } from 'react';
import { CodeBlock } from '@/components/code-block';
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/page';
import { OpenAPIPage } from '@/components/openapi-page';
import { openapi } from '@/lib/openapi';
import { SITE } from '@/lib/site';
import type { source } from '@/lib/source';
import { jsonLd } from '@/lib/json-ld';

type DocPage = NonNullable<ReturnType<typeof source.getPage>>;

/**
 * Every fence in the docs, rendered in the same shell as the landing page's
 * artifacts — one code block for the whole site, so a `docker run` line looks the
 * same whichever page it is on. Replaces fumadocs' own block, which has its own
 * chrome, its own copy affordance and no language label.
 *
 * The <pre> is wrapped, never replaced: it carries Shiki's inline theme colours,
 * and re-emitting the children as text collapses the whitespace. The language
 * comes off the <code> class that `addLanguageClass` puts there (see
 * source.config.ts); the clipboard text comes off the rendered <pre>, because by
 * this point the code is a tree of coloured spans and not a string.
 */
function MdxPre({
  children,
  title,
  // rehype-code attaches a language icon as a raw SVG string for fumadocs' own
  // block to render. We label the language as text, so this would otherwise be
  // spread onto the <pre> as an invalid attribute — an inline SVG per fence.
  icon: _icon,
  ...props
}: ComponentProps<'pre'> & { title?: string; icon?: string }) {
  const code = isValidElement<{ className?: string }>(children) ? children : undefined;
  const lang = /language-([\w-]+)/.exec(code?.props.className ?? '')?.[1];

  return (
    <CodeBlock lang={lang} title={title}>
      <pre {...props}>{children as ReactNode}</pre>
    </CodeBlock>
  );
}

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
            pre: MdxPre,
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
        dangerouslySetInnerHTML={{ __html: jsonLd(schema) }}
      />
    </DocsPage>
  );
}
