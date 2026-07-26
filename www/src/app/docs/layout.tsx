import Link from 'next/link';
import type { ReactNode } from 'react';
import { SiteFooter, SiteHeader } from '@/components/site-chrome';
import { DOCS } from '@/lib/docs';

/**
 * Same width, same chrome, same hairlines as the landing page — the header and
 * footer are literally the same components, so the two cannot drift apart. The
 * only thing docs add is the sidebar.
 */
export default function DocsLayout({ children }: { children: ReactNode }) {
  return (
    <div className="mx-auto max-w-[60rem] px-6">
      <SiteHeader />
      <div className="gap-10 border-t border-line pt-10 pb-16 sm:flex">
        <nav aria-label="Docs" className="mb-8 shrink-0 sm:mb-0 sm:w-44">
          <p className="mb-3 font-mono text-[0.6875rem] tracking-[0.2em] text-faint">DOCS</p>
          <ul className="flex flex-col gap-0.5 text-[0.8125rem]">
            {DOCS.map((doc) => (
              <li key={doc.url}>
                <Link
                  href={doc.url}
                  prefetch={false}
                  className="block rounded-md px-2 py-1.5 text-muted-ink hover:bg-surface hover:text-strong"
                >
                  {doc.title}
                </Link>
              </li>
            ))}
          </ul>
        </nav>
        <main id="main" className="min-w-0 flex-1">
          {children}
        </main>
      </div>
      <SiteFooter />
    </div>
  );
}
