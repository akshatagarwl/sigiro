import { source } from '@/app/source';
import { FAQ, SITE } from '@/lib/site';

/**
 * The index an agent reads before deciding whether we are worth its tokens.
 * Google has said it does not use llms.txt and the measured crawler demand is
 * ~0.1% of AI bot traffic — but Anthropic's crawler does fetch it, and this is
 * two dozen lines generated from the same data as the site.
 */
export function GET() {
  const docs = source
    .getPages()
    .map((page) => `- [${page.data.title}](${SITE.url}${page.url}): ${page.data.description ?? ''}`)
    .join('\n');

  const body = `# ${SITE.name}

> ${SITE.description}

sigiro covers detect and diagnose in a detect → diagnose → fix → verify → learn
loop. It is not a coding agent and not a CI system.

## Run it

\`\`\`
${SITE.install}
\`\`\`

Ports: 4317 OTLP/gRPC, 4318 OTLP/HTTP, 9999 query + investigate API.

## API

- POST /v1/traces, /v1/logs, /v1/metrics, /v1/profiles — OTLP ingest
- GET  /v1/anomalies — ranked baseline shifts, grouped by incident_id
- POST /v1/investigate — one structured evidence block per service
- POST /v1/query — SQL over your telemetry (DuckDB + Parquet)
- GET  /v1/services, /v1/traces/{trace_id}, /v1/health
- GET  /openapi.json — full machine-readable API surface, no auth

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
