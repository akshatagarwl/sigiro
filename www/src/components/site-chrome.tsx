import Link from 'next/link';
import { COMPACT } from '@/components/site/action';
import { AskAI } from '@/components/site/ask-ai';
import { Logo } from '@/components/site/logo';
import { ThemeToggle } from '@/components/site/theme-toggle';
import { NAV, SITE, SOCIAL } from '@/lib/site';

/**
 * One header and one footer for the whole site. The landing page and the docs
 * import these rather than each styling their own — that is the only way they
 * stay cohesive as either one changes.
 *
 * Both are compartments of the page panel, so they are divided from the content
 * with a hairline and never with a material: a border divides, a material lifts,
 * and these are neither above nor below what they sit beside.
 *
 * The header is `h-12`. That is not a spacing choice — it is the height the docs
 * sidebar's own header matches, which is what makes the two top edges line up.
 */
export function SiteHeader() {
  return (
    <header className="flex h-12 shrink-0 items-center gap-4 border-b border-hairline px-4 sm:px-5">
      <Link href="/" aria-label={`${SITE.name} home`} className="shrink-0 rounded-[6px]">
        <Logo />
      </Link>

      <nav aria-label="Main" className="flex shrink-0 items-center gap-1">
        {NAV.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            prefetch={false}
            /* Padded rather than taller, so the 24px hit area is reached without
               moving the text off the header's baseline. */
            className="press inline-flex h-7 items-center rounded-[6px] px-2 text-[12.5px] text-ink-3 hover:text-ink"
          >
            {item.label}
          </Link>
        ))}
      </nav>

      <span className="flex-1" />

      <ThemeToggle />

      {/* Below sm the hero's own action is one screen away at most, and this row
          has no width to spare for a second copy of it.

          The wrapper carries the breakpoint, not the link: COMPACT already sets
          `inline-flex`, and two unprefixed display utilities on one element are
          decided by their order in the generated stylesheet rather than by which
          one you wrote last — so `hidden` lost and the button showed on a phone
          anyway. */}
      <div className="hidden sm:block">
        <Link href="https://docs.sigiro.com/quickstart" prefetch={false} className={COMPACT}>
          Get started
        </Link>
      </div>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="flex flex-wrap items-center justify-between gap-x-6 gap-y-3 border-t border-hairline px-4 py-4 sm:px-5">
      {/* The mark and the ask-AI row are one group, so the row stays bottom-left
          beside the wordmark instead of drifting to the middle when the footer
          wraps. `gap-y-3` is what separates them once it does. */}
      <div className="flex flex-wrap items-center gap-x-5 gap-y-3">
        <Logo size={13} />
        <AskAI />
      </div>
      <nav
        aria-label="Footer"
        /* `*:` so every link gets the 24px hit area, including the one-character
           ones, without repeating the class five times. */
        className="flex flex-wrap items-center gap-x-3 gap-y-1 *:inline-flex *:min-w-6 *:items-center *:rounded-[5px]"
      >
        {NAV.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            prefetch={false}
            className="meta text-ink-3 hover:text-ink"
          >
            {item.label.toLowerCase()}
          </Link>
        ))}
        {SOCIAL.map((item) => (
          <a
            key={item.href}
            href={item.href}
            rel="me noopener"
            className="meta text-ink-3 hover:text-ink"
          >
            {item.label.toLowerCase()}
          </a>
        ))}
        {/* Plain anchor on purpose: /llms.txt is a text/plain route handler, and a
            client-side navigation would try to render it as a React page. */}
        <a href="/llms.txt" className="meta text-ink-3 hover:text-ink">
          llms.txt
        </a>
      </nav>
    </footer>
  );
}

/**
 * Every page is a panel floating on the bezel with a gutter around it. The gutter
 * is not spacing — it is the frame, and it is what tells you the panel is an
 * object rather than the page itself.
 *
 * `overflow-clip` and not `overflow-hidden`: both clip to the 20px radius, but
 * `hidden` makes this box a scroll container, and then every `position: sticky`
 * inside it resolves against a box that cannot scroll and silently stops
 * sticking. `clip` is not a scroll container, so the viewport stays the
 * scrollport.
 */
export function PageShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex flex-1 p-3 sm:p-5">
      <div className="mat-panel flex min-w-0 flex-1 flex-col overflow-clip rounded-[20px]">
        {children}
      </div>
    </div>
  );
}
