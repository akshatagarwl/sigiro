import { FAQ } from '@/lib/site';

/**
 * Native <details>. No client component, so this page ships no React runtime at
 * all — the whole reason mobile LCP is fast. The answers are in the served HTML
 * (crawlable, Ctrl+F-able, and present for the FAQPage JSON-LD), collapsed so the
 * page does not read as a wall of text, and they open with JavaScript disabled.
 */
export function Faq() {
  return (
    <div className="divide-y divide-neutral-200 border-y border-neutral-200 dark:divide-neutral-800 dark:border-neutral-800">
      {FAQ.map((item) => (
        <details key={item.q} className="group py-2">
          <summary className="flex cursor-pointer items-start justify-between gap-4 py-3 text-lg font-medium marker:content-none [&::-webkit-details-marker]:hidden">
            <h3 className="text-lg font-medium">{item.q}</h3>
            <span
              aria-hidden
              className="mt-1 shrink-0 text-neutral-500 transition-transform duration-200 group-open:rotate-45 dark:text-neutral-400"
            >
              +
            </span>
          </summary>
          <p className="max-w-2xl pb-3 text-neutral-700 dark:text-neutral-300">{item.a}</p>
        </details>
      ))}
    </div>
  );
}
