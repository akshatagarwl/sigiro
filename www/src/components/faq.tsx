import { Accordion } from '@/components/interior/accordion';
import { FAQ } from '@/lib/site';

/**
 * `interior/accordion`, one panel open at a time.
 *
 * Every answer is in the served HTML whatever its panel is doing — a closed panel
 * is a box animated to height 0 and marked `inert`, not a branch that was never
 * rendered. That is what the FAQPage JSON-LD on the page needs to be honoured:
 * schema whose text does not appear in the document gets ignored.
 *
 * `headingLevel` is 3 because the section above this renders the `h2`.
 */
export function Faq() {
  return (
    <Accordion
      className="mt-6"
      headingLevel={3}
      maxPanelHeight={320}
      items={FAQ.map((item, i) => ({
        id: `faq-${i}`,
        title: item.q,
        content: item.a,
      }))}
    />
  );
}
