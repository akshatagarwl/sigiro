/**
 * Generates the API reference pages from openapi.json — the same document sigiro
 * serves at /openapi.json and tells agents to read. The spec is a committed
 * snapshot (see openapi.json) so the build never depends on a running server;
 * refresh it with `just api-spec` when the surface changes.
 *
 * Run: bun run api:docs
 */
import { basename } from 'node:path';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { generateFiles } from 'fumadocs-openapi';
import { openapi } from '../src/lib/openapi';
import spec from '../openapi.json';

const OUT = './content/docs/api';

/** operationId → "POST /v1/query", for the index cards. */
const routes = new Map(
  Object.entries(spec.paths).flatMap(([path, methods]) =>
    Object.entries(methods as Record<string, { operationId?: string; summary?: string }>).map(
      ([method, op]) =>
        [
          op.operationId ?? '',
          { route: `${method.toUpperCase()} ${path}`, summary: op.summary ?? '' },
        ] as const,
    ),
  ),
);

await rm(OUT, { recursive: true, force: true });

await generateFiles({
  input: openapi,
  output: OUT,
  // One page per operation, flat. No `groupBy`: the index card writer skips
  // entries of type "group", so grouping by route made every entry a group and
  // the index page rendered an empty <Cards>. Five endpoints do not need a folder
  // each, and the file name is the operationId — which is why those are set
  // explicitly in the Rust #[utoipa::path] attributes rather than defaulting to
  // the handler function name.
  per: 'operation',
  // Writes the folder's meta.json. Without it the generated pages exist but
  // nothing links to them: the whole reference was unreachable from the sidebar.
  meta: true,
  index: {
    items: [
      {
        path: 'index.mdx',
        title: 'API reference',
        description: 'Every endpoint sigiro answers on, generated from the spec an agent reads.',
        // Without `only`, the index renders an empty <Cards>.
        only: ['./openapi.json'],
      },
    ],
    // A function, not `{baseUrl, contentDir}`: that form resolved the generated
    // paths against the wrong root and emitted `/docs/api/../../../run-query`.
    url: (file) => `/docs/api/${basename(file, '.mdx')}`,
  },
  beforeWrite(files) {
    // Descriptions in the spec are markdown, which is right for the document an
    // agent reads. But frontmatter `description` is rendered as plain text — under
    // the page title and inside <meta name="description"> — so the backticks show
    // up literally in both. Strip them from the frontmatter only; the spec and the
    // rendered body keep theirs.
    for (const file of files) {
      if (!file.path.endsWith('.mdx')) continue;
      // Only the frontmatter `description` — the rest of the frontmatter carries the
      // operation content that renders the page body, and that is markdown by
      // design.
      // Up to the next line that starts at column 0 — a top-level key, or the
      // closing `---`. YAML has several ways to write a long scalar and the
      // generator uses more than one: matching `description:` plus one line left
      // the backticks on every continuation line of a quoted block.
      const desc = /^description: [\s\S]*?(?=^\S)/m.exec(file.content);
      if (!desc) throw new Error(`no frontmatter description in ${file.path}`);
      file.content =
        file.content.slice(0, desc.index) +
        desc[0].replaceAll('`', '') +
        file.content.slice(desc.index + desc[0].length);
    }

    const index = files.find((f) => f.path.endsWith('index.mdx'));
    if (!index) throw new Error('index.mdx was not generated — check the index config');

    // The generator puts each operation's full spec description on its card. Ours
    // are written for an agent, so that is a dense paragraph per card and the page
    // becomes unreadable. The summary is the title; the route is the useful second
    // line, and it is what a reader is scanning for anyway.
    const cards = [...routes]
      .filter(([id]) => id)
      .map(
        ([id, { route, summary }]) =>
          `<Card href="/docs/api/${id}" title=${JSON.stringify(summary)} description=${JSON.stringify(route)} />`,
      );
    // `## Endpoints` is not decoration: each card's title is an <h3>, so without a
    // heading between them and the page title the document goes h1 -> h3.
    const rebuilt = index.content.replace(
      /<Cards>[\s\S]*<\/Cards>/,
      `## Endpoints\n\n<Cards>\n${cards.join('\n')}\n</Cards>`,
    );
    if (rebuilt === index.content) throw new Error('could not find the <Cards> block to rewrite');
    index.content = rebuilt;
  },
});

/**
 * A route file per operation.
 *
 * Under vinext, `generateMetadata` — sync or async — has its tags streamed into
 * <body>, where a crawler that does not run JavaScript never sees them. Only a
 * static `metadata` export reaches <head>. So every doc page needs its own route
 * file, and rather than hand-maintaining five near-identical ones next to a
 * catch-all that cannot carry metadata, they are written from the same spec that
 * generates the pages. Adding an endpoint in Rust produces its page, its sidebar
 * entry and its route in one command.
 */
const ROUTES = './src/app/docs/api';
const ids = [...routes.keys()].filter(Boolean);

for (const dir of ids) {
  await rm(`${ROUTES}/${dir}`, { recursive: true, force: true });
  await mkdir(`${ROUTES}/${dir}`, { recursive: true });
  await writeFile(
    `${ROUTES}/${dir}/page.tsx`,
    `// Generated by scripts/generate-api-docs.ts. Do not edit.
import { DocRoute } from '@/components/doc-route';
import { source } from '@/lib/source';

const page = source.getPage(['api', '${dir}'])!;

export const dynamic = 'force-static';

export const metadata = {
  title: page.data.title,
  description: page.data.description,
  alternates: { canonical: page.url },
};

export default function Page() {
  return <DocRoute page={page} />;
}
`,
  );
}

console.log('API reference generated into', OUT, '—', ids.length, 'routes');
