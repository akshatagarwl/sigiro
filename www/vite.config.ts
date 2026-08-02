import tailwindcss from '@tailwindcss/vite';
import vinext from 'vinext';
import { defineConfig } from 'vite';
import { cloudflare } from '@cloudflare/vite-plugin';

// defineConfig from 'vite', not 'vite-plus': the plugins are typed against
// vite's own types, and mixing the two makes tsc fail on the plugins array.

/**
 * No MDX plugin and no `optimizeDeps` block any more. Both existed for fumadocs
 * — the plugin compiled `content/docs`, and the exclude/include lists stopped
 * Vite pre-bundling fumadocs into duplicate React contexts. The docs moved to
 * Blume at docs.sigiro.com, so this app has no markdown left in it.
 */
export default defineConfig({
  plugins: [
    tailwindcss(),
    // No cdnAdapter: it set `cdn-cache-control: max-age=31536000` on the rendered
    // HTML, so after a deploy the edge kept serving last build's HTML pointing at
    // chunk hashes that no longer existed — 404s on every JS file and a broken
    // hydration. Every page here is prerendered and the assets have immutable
    // hashed URLs, so an HTML cache in front of that buys nothing.
    vinext(),
    // cloudflare() needs `viteEnvironment: { name: 'rsc', childEnvironments: ['ssr'] }`
    // — without it every route 404s in dev. `vinext init --platform=cloudflare`
    // generates this block; don't hand-edit it away.
    cloudflare({
      viteEnvironment: {
        name: 'rsc',
        childEnvironments: ['ssr'],
      },
    }),
  ],
});
