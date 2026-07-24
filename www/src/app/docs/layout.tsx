import Link from 'next/link';
import type { ReactNode } from 'react';
import { DOCS } from '@/lib/docs';
import { SITE } from '@/lib/site';

export default function DocsLayout({ children }: { children: ReactNode }) {
  return (
    <div className="mx-auto max-w-5xl px-6">
      <header className="flex items-center justify-between py-6">
        <Link href="/" className="font-mono text-lg font-semibold tracking-tight">
          {SITE.name}
        </Link>
        <span className="text-sm text-neutral-500 dark:text-neutral-400">Docs</span>
      </header>
      <div className="gap-12 pb-24 sm:flex">
        <nav aria-label="Docs" className="mb-8 shrink-0 sm:mb-0 sm:w-48">
          <ul className="space-y-1 text-sm">
            {DOCS.map((doc) => (
              <li key={doc.url}>
                <Link
                  href={doc.url}
                  className="block rounded px-2 py-1 text-neutral-700 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-900"
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
    </div>
  );
}
