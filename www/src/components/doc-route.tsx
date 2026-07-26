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
 * One body for every docs route. Every doc has its own route file — the four
 * hand-written ones by hand, the API reference generated alongside its MDX — and
 * that is not a stylistic choice: under vinext `generateMetadata` has its tags
 * streamed into `<body>`, where a crawler that does not run JavaScript never sees
 * them, so a static `metadata` export per route is the only way the title,
 * description and canonical reach `<head>`.
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
      {/* fumadocs sizes this at 18px with a 32px bottom margin, which on top of
          the prose paragraph's own top margin left a ~52px hole under every doc
          title. 17px is the landing page's lede size. */}
      <DocsDescription className="text-muted-ink mb-1 text-[1.0625rem]">
        {page.data.description}
      </DocsDescription>
      <DocsBody>
        <MDX
          components={{
            ...defaultMdxComponents,
            pre: MdxPre,
            // The layout container is `overflow-x-clip`, so a table wider than
            // the column does not make the page scrollable — it pushes the whole
            // document wider and the right-hand side is cut off, header included.
            // Every markdown table did this on a phone. The scroll box has to be
            // a wrapper: `display: block` on the table itself gives it one, but
            // then the rows shrink-wrap inside a full-width border.
            table: (props: ComponentProps<'table'>) => (
              <div className="my-6 overflow-x-auto">
                <table {...props} />
              </div>
            ),
            // The MDX supplies `document` and `operations`; `preloaded` carries
            // the resolved schema. Merged and cast once, because the prop type is
            // a union and neither half satisfies it alone.
            APIPage: (props: object) => (
              <OpenAPIPage {...({ ...props, ...preloaded } as OpenAPIPageProps)} />
            ),
          }}
        />
      </DocsBody>
      <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: jsonLd(schema) }} />
    </DocsPage>
  );
}
