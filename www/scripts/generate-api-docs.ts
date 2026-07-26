/**
 * Generates the API reference pages from openapi.json — the same document sigiro
 * serves at /openapi.json and tells agents to read. The spec is a committed
 * snapshot (see openapi.json) so the build never depends on a running server;
 * refresh it with `just api-spec` when the surface changes.
 *
 * Run: bun run api:docs
 */
import { rm } from 'node:fs/promises';
import { generateFiles } from 'fumadocs-openapi';
import { openapi } from '../src/lib/openapi';

const OUT = './content/docs/api';

await rm(OUT, { recursive: true, force: true });

await generateFiles({
  input: openapi,
  output: OUT,
  per: 'operation',
  groupBy: 'route',
  index: {
    items: [{ path: 'index.mdx', title: 'API reference', description: 'Every endpoint sigiro answers on, generated from the spec an agent reads.' }],
    url: { baseUrl: '/docs/api', contentDir: OUT },
  },
});

console.log('API reference generated into', OUT);
