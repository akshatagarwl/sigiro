import { defineConfig } from 'vite-plus'
import react from '@vitejs/plugin-react'
import mdx from '@mdx-js/rollup'
import remarkGfm from 'remark-gfm'
import remarkFrontmatter from 'remark-frontmatter'
import rehypeSlug from 'rehype-slug'
import rehypeAutolinkHeadings from 'rehype-autolink-headings'

export default defineConfig({
  plugins: [
    // Treat .canvas files as JSON (JSON Canvas spec)
    {
      name: 'canvas-json',
      transform(code: string, id: string) {
        if (id.endsWith('.canvas')) {
          return { code: `export default ${code}`, map: null }
        }
      },
    },
    {
      enforce: 'pre' as const,
      ...mdx({
        remarkPlugins: [remarkGfm, remarkFrontmatter],
        rehypePlugins: [rehypeSlug, rehypeAutolinkHeadings],
      }),
    },
    react({ include: /\.(tsx|ts|jsx|js|mdx|md)$/ }),
  ],
  lint: {
    ignorePatterns: ['dist/**', '**/*.mdx'],
    options: {
      typeAware: true,
      typeCheck: true,
    },
  },
  fmt: {
    singleQuote: true,
    semi: false,
    sortPackageJson: true,
  },
})
