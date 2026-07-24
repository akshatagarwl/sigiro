'use client';

import { Accordion } from '@base-ui-components/react/accordion';
import { FAQ } from '@/lib/site';

/**
 * Base UI Accordion. `hiddenUntilFound` renders collapsed panels with
 * `hidden="until-found"`, so every answer stays in the served HTML — findable by
 * Ctrl+F and by a crawler, which is what the FAQPage JSON-LD on this page needs
 * — while the page still reads as a list rather than a wall of text.
 * Accordion.Header renders the <h3>; do not nest another heading inside it.
 */
export function Faq() {
  return (
    <Accordion.Root
      hiddenUntilFound
      className="divide-y divide-neutral-200 border-y border-neutral-200 dark:divide-neutral-800 dark:border-neutral-800"
    >
      {FAQ.map((item, i) => (
        <Accordion.Item key={item.q} value={i} className="py-2">
          <Accordion.Header>
            <Accordion.Trigger className="flex w-full cursor-pointer items-start justify-between gap-4 py-3 text-left text-lg font-medium">
              {item.q}
              <span
                aria-hidden
                className="mt-1 shrink-0 text-neutral-500 transition-transform duration-200 data-[panel-open]:rotate-45 dark:text-neutral-400"
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
