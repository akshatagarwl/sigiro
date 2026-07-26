import Link from 'next/link';
import { NAV, SITE, SOCIAL } from '@/lib/site';

/**
 * One header and one footer for the whole site. The landing page and the docs
 * import these rather than each styling their own — that is the only way they
 * stay cohesive as either one changes.
 */

/** The changepoint glyph: a flat baseline that steps up. */
export function Mark({ size = 18 }: { size?: number }) {
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
      <path d="M5 24h7V10h15" />
    </svg>
  );
}

export function SiteHeader() {
  return (
    <header className="flex items-center justify-between py-5">
      <Link
        href="/"
        className="flex items-center gap-2 font-semibold tracking-tight text-strong"
      >
        <Mark />
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
        <a href="/llms.txt" className="text-faint hover:text-strong">
          llms.txt
        </a>
      </nav>
    </footer>
  );
}
