import { sample } from 'openapi-sampler';
import spec from '../../openapi.json';

/**
 * The API reference is rendered from our own OpenAPI document with our own
 * components. It used to go through fumadocs-openapi, which brings an interactive
 * playground, seven language snippets and a collapsible schema explorer — and its
 * own design system. Its `renderCodeblock` hook is not exposed on the public
 * render context, so its code samples could not be routed through the site's code
 * block, and keeping the two looking like one product meant a growing pile of CSS
 * overrides fighting utility classes in its markup.
 *
 * This spec has five operations, at most three parameters each, and response
 * schemas two or three levels deep. Reading it directly is less code than the glue
 * was, and everything then renders through one set of components.
 *
 * The one wheel worth keeping is schema → example JSON: `openapi-sampler` is the
 * library Redoc uses for exactly that, including `$ref` resolution.
 */

type Json = Record<string, unknown>;

export type Operation = {
  id: string;
  method: string;
  path: string;
  summary: string;
  description: string;
  secured: boolean;
  parameters: Parameter[];
  requestBody?: Body;
  responses: ResponseEntry[];
};

export type Parameter = {
  name: string;
  in: string;
  required: boolean;
  type: string;
  description: string;
};

export type Body = { mediaType: string; type: string; example: string };

export type ResponseEntry = {
  status: string;
  description: string;
  mediaType?: string;
  example?: string;
};

/** `["string","null"]` and `{type:"integer",format:"int64"}` both need naming. */
function typeName(schema: Json | undefined): string {
  if (!schema) return 'unknown';
  if (schema.$ref) return String(schema.$ref).split('/').pop() ?? 'object';
  const type = schema.type;
  const names = Array.isArray(type) ? type : type ? [String(type)] : [];
  const base = names.length ? names.join(' | ') : 'object';
  if (names.includes('array')) {
    const items = schema.items as Json | undefined;
    return `${typeName(items)}[]`;
  }
  return schema.format ? `${base} (${schema.format})` : base;
}

/**
 * An example payload, pretty-printed. A hand-written `example` in the spec wins:
 * sampling an untyped body (`Vec<serde_json::Value>`) yields `[null]`, which reads
 * as a broken endpoint rather than "rows, shaped by your SELECT".
 */
function exampleOf(schema: Json | undefined, given?: unknown): string | undefined {
  if (given !== undefined) return JSON.stringify(given, null, 2);
  if (!schema) return undefined;
  try {
    const value = sample(schema, { skipNonRequired: false }, spec as never);
    return value === undefined ? undefined : JSON.stringify(value, null, 2);
  } catch {
    // A sampler failure must not take a docs page down with it — the page is
    // still useful without an example, and the schema is in /openapi.json.
    return undefined;
  }
}

function content(holder: Json | undefined): {
  mediaType?: string;
  schema?: Json;
  example?: unknown;
} {
  const entries = Object.entries((holder?.content as Json | undefined) ?? {});
  const [mediaType, body] = entries[0] ?? [];
  const media = body as Json | undefined;
  return { mediaType, schema: media?.schema as Json | undefined, example: media?.example };
}

export function operations(): Operation[] {
  const out: Operation[] = [];

  for (const [path, methods] of Object.entries(spec.paths as Json)) {
    for (const [method, raw] of Object.entries(methods as Json)) {
      const op = raw as Json;
      const body = content(op.requestBody as Json | undefined);

      out.push({
        id: String(op.operationId),
        method: method.toUpperCase(),
        path,
        summary: String(op.summary ?? ''),
        description: String(op.description ?? ''),
        secured: Array.isArray(op.security) && op.security.length > 0,
        parameters: ((op.parameters as Json[]) ?? []).map((p) => ({
          name: String(p.name),
          in: String(p.in),
          required: Boolean(p.required),
          type: typeName(p.schema as Json),
          description: String(p.description ?? ''),
        })),
        requestBody: body.schema
          ? {
              mediaType: body.mediaType ?? 'application/json',
              type: typeName(body.schema),
              example: exampleOf(body.schema, body.example) ?? '',
            }
          : undefined,
        responses: Object.entries((op.responses as Json) ?? {}).map(([status, r]) => {
          const res = content(r as Json);
          return {
            status,
            description: String((r as Json).description ?? ''),
            mediaType: res.mediaType,
            example: exampleOf(res.schema, res.example),
          };
        }),
      });
    }
  }

  return out.sort((a, b) => a.id.localeCompare(b.id));
}

export function operation(id: string): Operation {
  const found = operations().find((o) => o.id === id);
  // Fail loudly: a missing id means the generated page and the spec disagree,
  // which is a build-time mistake, not something to paper over at runtime.
  if (!found) throw new Error(`no operation "${id}" in openapi.json`);
  return found;
}

/**
 * The simplest request that works, as a reader would paste it. Only *required*
 * query parameters go on the URL: the optional ones are documented in the table
 * directly below, and putting all of them here made the line long enough to clip.
 */
export function curl(op: Operation, baseUrl: string): string {
  const query = op.parameters
    .filter((p) => p.in === 'query' && p.required)
    .map((p) => `${p.name}=<${p.name}>`)
    .join('&');

  const lines = [`curl -X ${op.method} "${baseUrl}${op.path}${query ? `?${query}` : ''}"`];
  if (op.secured) lines.push(`  -H "Authorization: Bearer $SIGIRO_KEY"`);
  if (!op.requestBody) return lines.join(' \\\n');

  lines.push(`  -H "Content-Type: ${op.requestBody.mediaType}"`);
  const joined = lines.join(' \\\n');

  // The body stays pretty-printed inside the quotes. A single-quoted shell string
  // spans newlines, so this is still one paste-able command — and flattening a JSON
  // body onto one line made it 130 characters, which is a horizontal scroll on a
  // phone and unreadable on any screen.
  const body =
    op.requestBody.mediaType === 'text/plain'
      ? 'SELECT service_name, count(*) AS errors FROM sigiro_spans GROUP BY 1'
      : op.requestBody.example.replace(/\n/g, '\n  ');

  return `${joined} \\\n  -d '${body}'`;
}
