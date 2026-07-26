import matter from 'gray-matter';
import { Marked } from 'marked';
import markedShiki from 'marked-shiki';
import { highlight, resolveLang } from '@/lib/highlight';

/**
 * The whole docs pipeline: a glob, a frontmatter parse, and marked. Four markdown
 * files do not need a docs framework.
 *
 * Code fences go through the same Shiki highlighter the landing page uses, so a
 * `bash` block in the quickstart is coloured identically to the one in the hero.
 * That makes rendering async, which is why `DOCS` carries only metadata and the
 * HTML comes from `renderDoc` — the sitemap and llms.txt stay synchronous.
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

const marked = new Marked().use(
  markedShiki({
    async highlight(code, lang) {
      const resolved = resolveLang(lang);
      if (!resolved) {
        // No grammar for this fence. Emit the exact wrapper Shiki produces,
        // inline style included — relying on the cascade here left these blocks
        // with typography's gray-200 text on the page background, 1.2:1.
        const escaped = code
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;');
        return `<pre class="shiki github-light-default" style="background-color:#f4f4f3;color:#1f2328" tabindex="0"><code>${escaped}</code></pre>`;
      }
      return highlight(code, resolved);
    },
  }),
);

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
    return {
      slug,
      url: slug ? `/docs/${slug}` : '/docs',
      title,
      description,
      markdown: content,
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

export async function renderDoc(doc: Doc): Promise<string> {
  return marked.parse(doc.markdown, { async: true });
}
