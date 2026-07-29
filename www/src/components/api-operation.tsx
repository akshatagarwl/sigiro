import { CodeBlock } from '@/components/code-block';
import { curl, operation } from '@/lib/api-spec';
import { highlight } from '@/lib/highlight';
import { SITE } from '@/lib/site';

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

  // One await, not two in series: the request sample and the response samples do
  // not depend on each other, and highlighting them one after the other made the
  // page wait for both round trips instead of the slower one.
  const [requestHtml, responses] = await Promise.all([
    highlight(request, 'bash'),
    Promise.all(
      op.responses.map(async (r) => ({
        ...r,
        html: r.example ? await highlight(r.example, 'json') : undefined,
      })),
    ),
  ]);

  return (
    <div className="not-prose mt-8 flex flex-col gap-9 text-[0.875rem]">
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
                <tr className="border-b border-line text-[0.6875rem] tracking-[0.08em] text-faint uppercase">
                  <Th>Name</Th>
                  <Th>Type</Th>
                  <Th>Description</Th>
                </tr>
              </thead>
              <tbody>
                {op.parameters.map((p) => (
                  <tr key={p.name} className="border-b border-line align-baseline last:border-0">
                    <Td>
                      <Mono>{p.name}</Mono>
                      {p.required ? (
                        <span className="ml-1.5 text-[0.6875rem] text-faint">required</span>
                      ) : null}
                    </Td>
                    <Td>
                      <span className="font-mono text-[0.8125rem] text-muted-ink">{p.type}</span>
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
                <span className="font-mono text-[0.8125rem] font-medium text-strong">
                  {r.status}
                </span>
                <span className="text-muted-ink">{r.description}</span>
                {r.mediaType ? (
                  <span className="font-mono text-[0.6875rem] text-faint">{r.mediaType}</span>
                ) : null}
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

      <p className="text-[0.8125rem] text-faint">
        Every field and constraint is in{' '}
        <a href={`${SITE.apiUrl}/openapi.json`} className="text-muted-ink underline">
          /openapi.json
        </a>
        , which is the document this page is generated from.
      </p>
    </div>
  );
}

/** Section heading at the same size as an `h3` in the prose above it. */
function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section>
      <h2 className="mb-3 text-[0.9375rem] font-semibold tracking-[-0.02em] text-strong">
        {title}
      </h2>
      {children}
    </section>
  );
}

function Row({ term, children }: { term: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="font-mono text-[0.6875rem] tracking-[0.08em] text-faint uppercase">{term}</dt>
      <dd className="text-muted-ink">{children}</dd>
    </>
  );
}

function Mono({ children }: { children: React.ReactNode }) {
  return (
    <span className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[0.8125rem] text-strong">
      {children}
    </span>
  );
}

function Th({ children }: { children: React.ReactNode }) {
  return <th className="py-2 pr-5 font-normal last:pr-0">{children}</th>;
}

function Td({ children }: { children: React.ReactNode }) {
  return <td className="py-2.5 pr-5 last:pr-0">{children}</td>;
}
