import matter from 'gray-matter';
import { marked } from 'marked';

/**
 * The whole docs pipeline. Four markdown files do not need a docs framework —
 * fumadocs-ui shipped ~400 KB (search index, orama, remark, icon set) and scored
 * 74 on mobile for these pages. This is a glob, a frontmatter parse and marked.
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
  html: string;
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
      // llms.txt summary, and Lighthouse SEO drops for it.
      throw new Error(`content/docs/${name}.md is missing a frontmatter description`);
    }
    const slug = name === 'index' ? '' : name;
    return {
      slug,
      url: slug ? `/docs/${slug}` : '/docs',
      title,
      description,
      markdown: content,
      html: marked.parse(content, { async: false }),
    };
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
