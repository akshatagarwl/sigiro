import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import { RootProvider } from 'fumadocs-ui/provider/next';
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
 */
export default function Layout({ children }: { children: ReactNode }) {
  return (
    /* RootProvider lives here, not in the root layout: it carries the search
       dialog and theme context, and the landing page must not pay for either. */
    <RootProvider theme={{ enabled: false }}>
      <div className="mx-auto max-w-[72rem] px-6">
        <SiteHeader />
        <DocsLayout
          tree={source.pageTree}
          nav={{ enabled: false }}
          sidebar={{ collapsible: false }}
          containerProps={{ className: 'border-t border-line pt-6' }}
        >
          {/* fumadocs' DocsLayout renders no <main>, so the page had no landmark
              and the root layout's skip link pointed at nothing. */}
          <main id="main">{children}</main>
        </DocsLayout>
        <SiteFooter />
      </div>
    </RootProvider>
  );
}
