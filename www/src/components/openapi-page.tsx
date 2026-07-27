'use client';

import { createOpenAPIPage } from 'fumadocs-openapi/ui';

/**
 * `<OpenAPIPage />` is a client component the generated MDX calls as `APIPage`.
 * Created once here rather than per page so the playground code is one chunk.
 */
export const OpenAPIPage = createOpenAPIPage({
  /**
   * Off. It renders a card per request body offering a generated TypeScript type,
   * with its own Copy button competing with the sample panel's — and on this API
   * the bodies are a raw SQL string and a two-field JSON object, so the type it
   * offers is `string`. Nothing to hand a reader that the schema below it does not
   * already say.
   */
  generateTypeScriptDefinitions: false,
});
