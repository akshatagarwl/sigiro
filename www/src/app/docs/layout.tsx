import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { SidebarTrigger } from 'fumadocs-ui/layouts/docs/slots/sidebar';
import { RootProvider } from 'fumadocs-ui/provider/next';
import { PanelLeft } from 'lucide-react';
import type { ReactNode } from 'react';
import { SiteFooter, SiteHeader } from '@/components/site-chrome';
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
 * fumadocs' shell, wrapped in our own header and footer so the docs stay part of
 * the site rather than looking like a bolted-on framework. Theming is in
 * docs.css: fumadocs exposes its palette as CSS variables, which are mapped onto
 * the same tokens the landing page uses.
 *
 * Three things here are load-bearing, all for the same reason — `DocsLayout`
 * renders a five-column CSS grid with named areas (sidebar / header / main /
 * toc), and its children place *themselves* into those areas:
 *
 *  - DocsLayout is NOT inside a max-width container. It owns the full viewport
 *    width and centres itself with `--fd-layout-width`; boxing it in one nests
 *    padding and squeezes the content column to nothing on a phone.
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
      <div className="mx-auto w-full max-w-[78rem] px-4">
        <SiteHeader />
      </div>
      <DocsLayout
        tree={source.pageTree}
        nav={{ enabled: false }}
        sidebar={{ collapsible: false }}
        // The palette is light-only; a theme switch here just breaks the page.
        themeSwitch={{ enabled: false }}
        containerProps={{
          className: 'border-t border-line',
          // Match the landing page's measure, so a doc and the home page line up
          // in the same column rather than fumadocs' much wider 97rem default.
          style: { '--fd-layout-width': '78rem' } as React.CSSProperties,
        }}
      >
        <div className="flex items-center [grid-area:header] px-4 py-2.5 md:hidden">
          <SidebarTrigger className="flex items-center gap-2 rounded-lg border border-line px-2.5 py-1.5 text-[0.8125rem] text-muted-ink">
            <PanelLeft className="size-4" aria-hidden />
            Docs menu
          </SidebarTrigger>
        </div>
        {/* `contents` and not a box: see the note above. */}
        <main id="main" className="contents">
          {children}
        </main>
      </DocsLayout>
      <div className="mx-auto w-full max-w-[78rem] px-4">
        <SiteFooter />
      </div>
    </RootProvider>
  );
}
