import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { SidebarTrigger } from 'fumadocs-ui/layouts/docs/slots/sidebar';
import { RootProvider } from 'fumadocs-ui/provider/next';
import type { ReactNode } from 'react';
import { PageShell, SiteFooter, SiteHeader } from '@/components/site-chrome';
import type { Metadata } from 'next';
import { SITE } from '@/lib/site';
import { source } from '@/lib/source';
import './docs.css';

/**
 * A static description for every docs route. The per-page one from
 * `generateMetadata` is more specific but lands in <body> under vinext, so
 * without this there is no head-level description at all.
 */
export const metadata: Metadata = {
  description: `Docs for ${SITE.name}: quickstart, agent instrumentation, hosted onboarding, and the full API reference generated from the spec sigiro serves.`,
};

/**
 * fumadocs' shell, inside the same page panel the landing page uses, so the docs
 * are a compartment of this site rather than a bolted-on framework. Theming is in
 * docs.css: fumadocs exposes its palette as CSS variables, and every one of them
 * is mapped onto a material or an ink.
 *
 * The sidebar is a compartment of the panel divided by a hairline, not a lifted
 * card of its own: it is furniture, not content, and a border is what divides one
 * compartment from another.
 *
 * Three things here are load-bearing, all for the same reason — `DocsLayout`
 * renders a five-column CSS grid with named areas (sidebar / header / main /
 * toc), and its children place *themselves* into those areas:
 *
 *  - DocsLayout is NOT inside a max-width container. It owns the full panel width
 *    and centres itself with `--fd-layout-width`; boxing it in one nests padding
 *    and squeezes the content column to nothing on a phone.
 *  - `<main>` is `display: contents`, so the article and the table of contents
 *    stay direct grid children. A normal wrapper swallows their `grid-area` and
 *    the whole page collapses into one implicit row.
 *  - The sidebar is `max-md:hidden` and opens as a drawer from a trigger that
 *    normally lives in fumadocs' own nav bar. That bar is disabled here, so
 *    without this trigger a phone has no way into the docs tree at all.
 */
export default function Layout({ children }: { children: ReactNode }) {
  return (
    /* RootProvider lives here, not in the root layout: it carries the search
       dialog and theme context, and the landing page must not pay for either. */
    <RootProvider theme={{ enabled: false }}>
      <PageShell>
        <SiteHeader />
        <DocsLayout
          tree={source.pageTree}
          nav={{ enabled: false }}
          sidebar={{ collapsible: false }}
          // The palette follows the one theme toggle in the site header, which
          // writes `.dark` on <html> itself. A second switch here would be a
          // second source of truth for the same class.
          themeSwitch={{ enabled: false }}
          containerProps={{
            // The panel is already the measure and its gutter is already the
            // frame, so the docs fill it rather than centring a narrower column
            // inside something that is itself centred.
            style: { '--fd-layout-width': '100%' } as React.CSSProperties,
          }}
        >
          <div className="flex items-center px-4 py-2.5 [grid-area:header] md:hidden">
            <SidebarTrigger className="mat-cap press inline-flex h-7 items-center gap-2 rounded-[7px] px-2.5 text-[12.5px] font-medium text-ink-2 hover:text-ink">
              {/* Drawn inline on the Phosphor grid: one glyph is not worth an icon
                  package. strokeWidth 16 on a 256 viewBox is 1.5px at this size. */}
              <svg
                width="14"
                height="14"
                viewBox="0 0 256 256"
                fill="none"
                stroke="currentColor"
                strokeWidth="16"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden
              >
                <rect x="32" y="48" width="192" height="160" rx="12" />
                <path d="M104 48v160" />
              </svg>
              Docs menu
            </SidebarTrigger>
          </div>
          {/* `contents` and not a box: see the note above. */}
          <main id="main" className="contents">
            {children}
          </main>
        </DocsLayout>
        <SiteFooter />
      </PageShell>
    </RootProvider>
  );
}
