'use client';

import { createOpenAPIPage } from 'fumadocs-openapi/ui';

/**
 * `<OpenAPIPage />` is a client component the generated MDX calls as `APIPage`.
 * Created once here rather than per page so the playground code is one chunk.
 */
export const OpenAPIPage = createOpenAPIPage();
