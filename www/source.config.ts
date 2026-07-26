import { defineConfig, defineDocs } from 'fumadocs-mdx/config';

export const docs = defineDocs({
  dir: 'content/docs',
  docs: {
    // Hand-written docs are .md; the generated API reference is .mdx.
    files: ['**/*.md', '**/*.mdx'],
    postprocess: { includeProcessedMarkdown: true },
  },
});

export default defineConfig({
  mdxOptions: {
    rehypeCodeOptions: {
      // Same theme as the landing page artifacts, so a fence looks identical on
      // both sides of the site. The two replaced colours are the theme's comment
      // grey and keyword red, which are 4.37:1 and 4.49:1 on our code background
      // — just under AA at this size.
      themes: { light: 'github-light-default', dark: 'github-light-default' },
      colorReplacements: {
        '#6a737d': '#5a6169',
        '#6e7781': '#5a6169',
        '#cf222e': '#b3121f',
      },
    },
  },
});
