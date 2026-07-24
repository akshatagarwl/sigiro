import { DocsLayout } from 'fumadocs-ui/layouts/notebook';
import { RootProvider } from 'fumadocs-ui/provider/next';
import type { ReactNode } from 'react';
import { source } from '@/app/source';
import { SITE } from '@/lib/site';
import './docs.css';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <RootProvider>
      <DocsLayout
        tree={source.pageTree}
        nav={{ title: SITE.name, url: '/' }}
      >
        {children}
      </DocsLayout>
    </RootProvider>
  );
}
