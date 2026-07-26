'use client';

import { Accordion } from '@base-ui/react/accordion';
import { FAQ } from '@/lib/site';

/**
 * Base UI Accordion. `hiddenUntilFound` renders collapsed panels with
 * `hidden="until-found"`, so every answer stays in the served HTML — findable by
 * Ctrl+F and by a crawler, which is what the FAQPage JSON-LD needs to be
 * honoured. Accordion.Header renders the heading; never nest another inside it.
 */
export function Faq() {
  return (
    <Accordion.Root hiddenUntilFound className="mt-6 border-t border-line">
      {FAQ.map((item, i) => (
        <Accordion.Item key={item.q} value={i} className="border-b border-line">
          <Accordion.Header>
            <Accordion.Trigger className="flex w-full cursor-pointer items-start justify-between gap-4 py-3.5 text-left font-medium text-strong">
              {item.q}
              <span aria-hidden className="mt-0.5 shrink-0 font-mono text-faint">
                +
              </span>
            </Accordion.Trigger>
          </Accordion.Header>
          <Accordion.Panel>
            <p className="max-w-[40rem] pb-4 text-sm text-muted-ink">{item.a}</p>
          </Accordion.Panel>
        </Accordion.Item>
      ))}
    </Accordion.Root>
  );
}
