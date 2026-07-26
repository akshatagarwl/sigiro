import matter from 'gray-matter';

/**
 * The whole docs pipeline: a glob and a frontmatter parse. Four markdown files do
 * not need a docs framework.
 *
 * Rendering lives in `components/doc-page.tsx` — react-markdown turns fences into
 * React elements, which is what lets every code block in the docs be the same
 * component (copy button included) as the ones on the landing page.
 */
const files = import.meta.glob('../../content/docs/*.md', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

export type Doc = {
  /** URL slug; the index page is '' so it lives at /docs. */
  slug: string;
  url: string;
  title: string;
  description: string;
  markdown: string;
};

/** Sidebar and sitemap order — anything unlisted sorts after, alphabetically. */
const ORDER = ['index', 'quickstart', 'install-with-ai', 'hosted-onboarding'];

function build(): Doc[] {
  const docs = Object.entries(files).map(([path, raw]) => {
    const name = path.split('/').pop()!.replace(/\.md$/, '');
    const { data, content } = matter(raw);
    const title = typeof data.title === 'string' ? data.title : name;
    const description = typeof data.description === 'string' ? data.description : '';
    if (!description) {
      // Fail loud: a doc with no description has no meta description and no
      // llms.txt summary, and Lighthouse SEO drops for that page.
      throw new Error(`content/docs/${name}.md is missing a frontmatter description`);
    }
    const slug = name === 'index' ? '' : name;
    return { slug, url: slug ? `/docs/${slug}` : '/docs', title, description, markdown: content };
  });

  return docs.sort((a, b) => {
    const rank = (d: Doc) => {
      const i = ORDER.indexOf(d.slug || 'index');
      return i === -1 ? ORDER.length : i;
    };
    return rank(a) - rank(b) || a.slug.localeCompare(b.slug);
  });
}

export const DOCS: Doc[] = build();

export function getDoc(slug: string | undefined): Doc | undefined {
  return DOCS.find((d) => d.slug === (slug ?? ''));
}
