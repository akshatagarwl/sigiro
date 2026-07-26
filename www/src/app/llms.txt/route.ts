import { FAQ, INSTALL, SITE } from '@/lib/site';
import { source } from '@/lib/source';

/**
 * The index an agent reads before deciding whether we are worth its tokens.
 * Google has said it does not use llms.txt and measured crawler demand is ~0.1%
 * of AI bot traffic — but Anthropic's crawler does fetch it, and this is two
 * dozen lines generated from the same source the docs render from.
 */
export const dynamic = 'force-static';

export function GET() {
  const docs = source
    .getPages()
    .map((page) => `- [${page.data.title}](${SITE.url}${page.url}): ${page.data.description ?? ''}`)
    .join('\n');

  const body = `# ${SITE.name}

> ${SITE.description}

## Run it

\`\`\`
${INSTALL[0].code}
\`\`\`

Ports: 4317 OTLP/gRPC, 4318 OTLP/HTTP, 9999 query + investigate API.

## API

The full machine-readable surface is at ${SITE.url}/openapi.json — no key required.

- GET  /v1/anomalies — ranked shifts against each service's own baseline
- POST /v1/investigate — one structured evidence block per service
- POST /v1/query — SQL over your telemetry
- GET  /v1/services, /v1/traces/{trace_id}, /v1/health

## Docs

${docs}
- [Full text of every page](${SITE.url}/llms-full.txt): all docs as markdown, one file

## FAQ

${FAQ.map((item) => `### ${item.q}\n\n${item.a}`).join('\n\n')}
`;

  return new Response(body, {
    headers: {
      'content-type': 'text/plain; charset=utf-8',
      'cache-control': 'public, max-age=3600',
    },
  });
}
