import { createOpenAPI } from 'fumadocs-openapi/server';
import spec from '../../openapi.json';

/**
 * The spec is imported as a module, not read from disk. fumadocs-openapi's
 * default `input: ['./openapi.json']` calls the filesystem, which does not exist
 * in the Workers runtime — the reference pages render under Node and 500 on
 * Cloudflare. Bundling it also means the docs can never disagree with the file.
 *
 * It is a committed snapshot of what `sigiro serve` returns at /openapi.json,
 * filtered to the /v1 surface: the admin endpoints that mint tenant keys are
 * deliberately not published.
 */
export const openapi = createOpenAPI({
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- the spec is a
  // generated document; fumadocs types it as its own Document union.
  // Key must match the `document` the generator wrote into each MDX page
  // verbatim — './openapi.json', not 'openapi.json', or the lookup misses and
  // every operation page fails.
  input: { './openapi.json': spec as any },
});
