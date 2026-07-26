import { docs } from 'collections/server';
import { loader } from 'fumadocs-core/source';
import { openapi } from '@/lib/openapi';

/**
 * One tree for the hand-written docs and the generated API reference.
 * `openapi.loaderPlugin()` is what attaches the method badges (GET/POST) and the
 * per-operation metadata to the generated pages.
 */
export const source = loader({
  baseUrl: '/docs',
  source: docs.toFumadocsSource(),
  plugins: [openapi.loaderPlugin()],
});
