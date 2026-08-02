import { Tab, Tabs } from 'fumadocs-ui/components/tabs';
import defaultMdxComponents from 'fumadocs-ui/mdx';
import { isValidElement, type ComponentProps, type ReactNode } from 'react';
import { CodeBlock } from '@/components/code-block';
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from 'fumadocs-ui/page';
import { ApiOperation } from '@/components/api-operation';
import { SITE } from '@/lib/site';
import type { source } from '@/lib/source';
import { jsonLd } from '@/lib/json-ld';

type DocPage = NonNullable<ReturnType<typeof source.getPage>>;

/**
 * The text of a fence, read back off the element tree Shiki produced.
 *
 * By the time MDX renders a fence the code is a tree of coloured spans, not a
 * string — and the copy cap needs a string. Walking the tree gets the exact text
 * the reader sees, and it does it on the server, so nothing has to read the DOM
 * back out of the rendered page afterwards.
 */
function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return '';
}

/**
 * Every fence in the docs, rendered in the same shell as the landing page's
 * artifacts — one code block for the whole site, so a `docker run` line looks the
 * same whichever page it is on. Replaces fumadocs' own block, which has its own
 * chrome, its own copy affordance and no language label.
 *
 * The <pre> is wrapped, never replaced: it carries Shiki's per-token theme
 * variables, and re-emitting the children as text collapses the whitespace. The
 * language comes off the <code> class that `addLanguageClass` puts there (see
 * source.config.ts).
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
    <CodeBlock lang={lang} title={title} value={textOf(children)}>
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
export function DocRoute({ page }: { page: DocPage }) {
  const MDX = page.data.body;

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
          title. Body size, not a lede size: on a generated reference page this is
          the operation's whole description and can run to a full paragraph. */}
      <DocsDescription className="mb-1 text-[13.5px] text-ink-2">
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
            // The generated reference pages call this for the mechanical part of
            // an operation; the prose above it is ordinary markdown.
            ApiOperation,
            /**
             * Binary or container, and the reader's answer is remembered.
             *
             * `groupId="sigiro-install"` with `persist` writes the choice to
             * `localStorage['sigiro-install']`. The landing page's terminal reads
             * that same key, so picking one here is still picked there — one
             * preference across the site, and no synchronising code, because
             * fumadocs already stores it under a key we choose.
             *
             * A doc that uses these has to be `.mdx`. `.md` is compiled with JSX
             * off, so the tags would render as literal text.
             */
            Tab,
            Tabs,
          }}
        />
      </DocsBody>
      <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: jsonLd(schema) }} />
    </DocsPage>
  );
}
