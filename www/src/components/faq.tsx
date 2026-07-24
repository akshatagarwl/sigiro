'use client';

import { Accordion } from '@base-ui-components/react/accordion';
import { FAQ } from '@/lib/site';

/**
 * Every answer ships expanded and stays in the DOM when collapsed
 * (`hiddenUntilFound`). Both matter: the FAQPage JSON-LD on this page is only
 * honoured while its text is actually present and visible to a reader.
 */
export function Faq() {
  return (
    <Accordion.Root
      defaultValue={FAQ.map((_, i) => i)}
      hiddenUntilFound
      className="divide-y divide-neutral-200 border-y border-neutral-200 dark:divide-neutral-800 dark:border-neutral-800"
    >
      {FAQ.map((item, i) => (
        <Accordion.Item key={item.q} value={i} className="py-2">
          <Accordion.Header>
            <Accordion.Trigger className="flex w-full items-start justify-between gap-4 py-3 text-left text-lg font-medium">
              <h3>{item.q}</h3>
              <span
                aria-hidden
                className="mt-1 shrink-0 text-neutral-400 transition-transform duration-200 data-[panel-open]:rotate-45"
              >
                +
              </span>
            </Accordion.Trigger>
          </Accordion.Header>
          <Accordion.Panel className="pb-3 text-neutral-700 dark:text-neutral-300">
            <p className="max-w-2xl">{item.a}</p>
          </Accordion.Panel>
        </Accordion.Item>
      ))}
    </Accordion.Root>
  );
}
