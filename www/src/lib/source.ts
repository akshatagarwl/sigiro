import { docs } from 'collections/server';
import { loader } from 'fumadocs-core/source';

/** One tree for the hand-written docs and the generated API reference. */
export const source = loader({
  baseUrl: '/docs',
  source: docs.toFumadocsSource(),
});
