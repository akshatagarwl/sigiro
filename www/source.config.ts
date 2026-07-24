import { metaSchema, pageSchema } from 'fumadocs-core/source/schema';
import { defineConfig, defineDocs } from 'fumadocs-mdx/config';

export const docs = defineDocs({
  dir: 'content/docs',
  docs: {
    files: ['**/*.md'],
    schema: pageSchema,
    // Keeps the processed markdown around so /llms-full.txt can serve real
    // source to agents instead of scraped HTML.
    postprocess: { includeProcessedMarkdown: true },
  },
  meta: { schema: metaSchema },
});

export default defineConfig();
