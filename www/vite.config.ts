import tailwindcss from '@tailwindcss/vite';
import mdx from 'fumadocs-mdx/vite';
import vinext from 'vinext';
import { defineConfig } from 'vite-plus';
import * as MdxConfig from './source.config.js';

export default defineConfig({
  // fumadocs must not be pre-bundled or its React contexts get duplicated.
  // Shape copied from cloudflare/vinext examples/fumadocs-docs-template.
  optimizeDeps: {
    exclude: ['fumadocs-ui', 'fumadocs-core'],
    include: [
      'fumadocs-ui > debug',
      'fumadocs-core > extend',
      'fumadocs-mdx > extend',
      'fumadocs-core > style-to-js',
      'fumadocs-mdx > style-to-js',
      // we never import next/image ourselves, but fumadocs does
      '@unpic/react',
    ],
  },
  // No cloudflare() plugin here on purpose: `@vinext/cloudflare deploy` wires
  // the Workers build itself, and adding the plugin without a wrangler entry
  // makes every route 404 in dev.
  plugins: [tailwindcss(), mdx(MdxConfig), vinext()],
});
