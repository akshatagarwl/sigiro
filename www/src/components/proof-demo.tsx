import { Effect } from 'effect';
import { ProofStage } from '@/components/proof-stage';
import { highlight } from '@/lib/highlight';
import { PROOF } from '@/lib/site';

/**
 * Server half of the 04 Prove demo: Shiki runs here, so no highlighting
 * JavaScript reaches the browser and the client half only ever animates markup
 * it was handed. Same division as `Artifact`.
 */
export async function ProofDemo({ finding, lang }: { finding: string; lang: 'json' }) {
  const [findingHtml, sqlHtml, ...rowsHtml] = await Effect.runPromise(
    Effect.all(
      [
        highlight(finding, lang),
        highlight(PROOF.sql, 'sql'),
        ...PROOF.rows.map((row) => highlight(row, 'json')),
      ],
      { concurrency: 'unbounded' },
    ),
  );

  const frag = (html: string) => <div dangerouslySetInnerHTML={{ __html: html }} />;

  return (
    <ProofStage
      finding={frag(findingHtml)}
      sql={frag(sqlHtml)}
      sqlSource={PROOF.sql}
      rows={rowsHtml.map((html) => frag(html))}
    />
  );
}
