import Link from 'next/link';
import { NAV, SITE, SOCIAL } from '@/lib/site';

/**
 * One header and one footer for the whole site. The landing page and the docs
 * import these rather than each styling their own — that is the only way they
 * stay cohesive as either one changes.
 */

/**
 * The changepoint glyph: a flat baseline that steps up. With `pulse`, a bright
 * segment travels the whole path — a signal passing through the mark, which is
 * the product's subject rather than decoration.
 *
 * Two copies of the same path: a faint base, and a dashed overlay whose offset
 * animates. `pathLength="100"` normalises the geometry so the dash lengths are
 * percentages and stay correct if the path ever changes.
 */
function Mark({ size = 18, pulse = false }: { size?: number; pulse?: boolean }) {
  const d = 'M5 24h7V10h15';
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      stroke="currentColor"
      strokeWidth="3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={d} className={pulse ? 'opacity-25' : undefined} />
      {pulse ? <path d={d} pathLength="100" className="mark-trace" /> : null}
    </svg>
  );
}

export function SiteHeader() {
  return (
    /* `gap-6` and not `justify-between` alone: with only the latter, a narrow
       viewport shrinks the free space to zero and the wordmark ends up touching
       the first nav link. */
    <header className="flex items-center justify-between gap-6 py-5">
      <Link
        href="/"
        className="flex shrink-0 items-center gap-2 font-semibold tracking-tight text-strong"
      >
        <Mark pulse />
        {SITE.name}
      </Link>
      <nav aria-label="Main" className="flex items-center gap-5 text-[0.8125rem]">
        {NAV.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            prefetch={false}
            className="text-muted-ink hover:text-strong"
          >
            {item.label}
          </Link>
        ))}
        <Link
          href="/docs/quickstart"
          prefetch={false}
          className="rounded-lg border border-line-strong px-3 py-1.5 font-medium text-strong"
        >
          Get started
        </Link>
      </nav>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="flex flex-wrap items-end justify-between gap-6 border-t border-line py-10">
      <div>
        <p className="font-semibold tracking-tight text-strong">{SITE.tagline}</p>
        <Link
          href="/docs/quickstart"
          prefetch={false}
          className="mt-3.5 inline-block rounded-lg bg-accent px-3.5 py-2 text-[0.8125rem] font-medium text-on-accent"
        >
          Get started
        </Link>
      </div>
      <nav aria-label="Footer" className="flex flex-wrap gap-4 font-mono text-[0.6875rem]">
        {NAV.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            prefetch={false}
            className="text-faint hover:text-strong"
          >
            {item.label.toLowerCase()}
          </Link>
        ))}
        {SOCIAL.map((item) => (
          <a
            key={item.href}
            href={item.href}
            rel="me noopener"
            className="text-faint hover:text-strong"
          >
            {item.label.toLowerCase()}
          </a>
        ))}
        {/* Plain anchor on purpose: /llms.txt is a text/plain route handler, and a
            client-side navigation would try to render it as a React page. */}
        <a href="/llms.txt" className="text-faint hover:text-strong">
          llms.txt
        </a>
      </nav>
    </footer>
  );
}
