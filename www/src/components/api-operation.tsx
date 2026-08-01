import { Effect } from 'effect';
import { CodeBlock } from '@/components/code-block';
import { curl, operation, type ResponseEntry } from '@/lib/api-spec';
import { highlight } from '@/lib/highlight';
import { SITE } from '@/lib/site';

/** A response entry with its example highlighted, where it has one. */
function withExample(r: ResponseEntry) {
  return r.example
    ? Effect.map(highlight(r.example, 'json'), (html) => ({
        ...r,
        html: html as string | undefined,
      }))
    : Effect.succeed({ ...r, html: undefined as string | undefined });
}

/**
 * One operation of the API reference, rendered from openapi.json with the site's
 * own components: the same CodeBlock as the landing page and the quickstart, the
 * same type ramp, the same tokens. See lib/api-spec.ts for why this is not
 * fumadocs-openapi.
 *
 * Server component — Shiki runs at request time on the Worker and no highlighting
 * JavaScript reaches the browser.
 */
export async function ApiOperation({ id }: { id: string }) {
  const op = operation(id);
  const request = curl(op, SITE.apiUrl);

  // One run, not two in series: the request sample and the response samples do
  // not depend on each other, and highlighting them one after the other made the
  // page wait for both round trips instead of the slower one.
  const [requestHtml, responses] = await Effect.runPromise(
    Effect.all(
      [
        highlight(request, 'bash'),
        Effect.all(op.responses.map(withExample), { concurrency: 'unbounded' }),
      ],
      { concurrency: 'unbounded' },
    ),
  );

  return (
    <div className="not-prose mt-8 flex flex-col gap-9 text-[13.5px] text-ink-2">
      <Section title="Request">
        <CodeBlock lang="bash" title={`${op.method} ${op.path}`} value={request}>
          {/* Shiki output. The input is openapi.json — a committed snapshot of our
              own spec — run through a highlighter, so there is no user-controlled
              string anywhere in this markup. */}
          <div dangerouslySetInnerHTML={{ __html: requestHtml }} />
        </CodeBlock>
        <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-5 gap-y-1.5">
          <Row term="Auth">
            {op.secured ? (
              <>
                <Mono>Authorization: Bearer &lt;key&gt;</Mono> — a tenant key. Self-hosted servers
                in open mode accept any value.
              </>
            ) : (
              'None. Self-hosted default; the hosted service requires a tenant key.'
            )}
          </Row>
          {op.requestBody ? (
            <Row term="Body">
              <Mono>{op.requestBody.mediaType}</Mono> — {op.requestBody.type}
            </Row>
          ) : null}
        </dl>
      </Section>

      {op.parameters.length > 0 ? (
        <Section title="Parameters">
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-left">
              <thead>
                <tr className="border-b border-hairline">
                  <Th>Name</Th>
                  <Th>Type</Th>
                  <Th>Description</Th>
                </tr>
              </thead>
              <tbody>
                {op.parameters.map((p) => (
                  <tr
                    key={p.name}
                    className="border-b border-hairline align-baseline last:border-0"
                  >
                    <Td>
                      <Mono>{p.name}</Mono>
                      {p.required ? (
                        <span className="ml-1.5 text-[11.5px] text-ink-3">required</span>
                      ) : null}
                    </Td>
                    <Td>
                      <span className="font-mono text-[12.5px] text-ink-2">{p.type}</span>
                    </Td>
                    <Td>{p.description}</Td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Section>
      ) : null}

      <Section title="Responses">
        <div className="flex flex-col gap-5">
          {responses.map((r) => (
            <div key={r.status}>
              <p className="flex flex-wrap items-baseline gap-2.5">
                <span className="font-mono text-[12.5px] font-medium text-ink">{r.status}</span>
                <span className="text-ink-2">{r.description}</span>
                {r.mediaType ? <span className="meta text-ink-3">{r.mediaType}</span> : null}
              </p>
              {r.html && r.example ? (
                <CodeBlock lang="json" meta="example" value={r.example}>
                  <div dangerouslySetInnerHTML={{ __html: r.html }} />
                </CodeBlock>
              ) : null}
            </div>
          ))}
        </div>
      </Section>

      <p className="text-[12.5px] text-ink-3">
        Every field and constraint is in{' '}
        <a
          href={`${SITE.apiUrl}/openapi.json`}
          className="text-accent underline decoration-accent/40 underline-offset-2"
        >
          /openapi.json
        </a>
        , which is the document this page is generated from.
      </p>
    </div>
  );
}

/**
 * A section heading here is a column header, not a voice in the prose: "Request",
 * "Parameters", "Responses" label the mechanical part of the page. That is one of
 * the three places this language allows uppercase, and `.label` carries the
 * tracking that keeps it from reading as a wall.
 */
function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section>
      <h2 className="label mb-3">{title}</h2>
      {children}
    </section>
  );
}

function Row({ term, children }: { term: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="label">{term}</dt>
      <dd className="text-ink-2">{children}</dd>
    </>
  );
}

function Mono({ children }: { children: React.ReactNode }) {
  return (
    <span className="mat-well rounded-[5px] px-1.5 py-0.5 font-mono text-[12.5px] text-ink">
      {children}
    </span>
  );
}

function Th({ children }: { children: React.ReactNode }) {
  return <th className="label py-2 pr-5 text-left last:pr-0">{children}</th>;
}

function Td({ children }: { children: React.ReactNode }) {
  return <td className="py-2.5 pr-5 last:pr-0">{children}</td>;
}
