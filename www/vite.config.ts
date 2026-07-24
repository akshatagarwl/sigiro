import tailwindcss from '@tailwindcss/vite';
import vinext from 'vinext';
import { defineConfig } from 'vite-plus';
import { cloudflare } from '@cloudflare/vite-plugin';

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
