import type { Metadata } from 'next';
import type { ReactNode } from 'react';
import { SITE } from '@/lib/site';
import './globals.css';

export const metadata: Metadata = {
  metadataBase: new URL(SITE.url),
  title: {
    default: SITE.title,
    template: `%s — ${SITE.name}`,
  },
  description: SITE.description,
  applicationName: SITE.name,
  alternates: { canonical: '/' },
  openGraph: {
    type: 'website',
    url: SITE.url,
    siteName: SITE.name,
    title: SITE.title,
    description: SITE.description,
    images: [{ url: '/og.png', width: 1200, height: 630, alt: SITE.tagline }],
  },
  twitter: {
    card: 'summary_large_image',
    site: '@SigiroAI',
    title: SITE.title,
    description: SITE.description,
    images: ['/og.png'],
  },
  robots: { index: true, follow: true },
};

/**
 * Sitewide entity graph. One @graph with cross-references beats three loose
 * blobs: it tells an answer engine that the organisation, the site and the
 * software are one entity.
 */
const graph = {
  '@context': 'https://schema.org',
  '@graph': [
    {
      '@type': 'Organization',
      '@id': `${SITE.url}/#organization`,
      name: SITE.name,
      url: SITE.url,
      description: SITE.description,
      sameAs: ['https://x.com/SigiroAI', 'https://www.linkedin.com/company/sigiro'],
    },
    {
      '@type': 'WebSite',
      '@id': `${SITE.url}/#website`,
      url: SITE.url,
      name: SITE.name,
      description: SITE.description,
      publisher: { '@id': `${SITE.url}/#organization` },
      inLanguage: 'en',
    },
    {
      '@type': 'SoftwareApplication',
      '@id': `${SITE.url}/#software`,
      name: SITE.name,
      applicationCategory: 'DeveloperApplication',
      applicationSubCategory: 'Observability',
      operatingSystem: 'Linux, macOS',
      url: SITE.url,
      description: SITE.description,
      featureList: [
        'OpenTelemetry OTLP ingest for traces, logs, metrics and profiles',
        'Threshold-free anomaly detection against each service’s own baseline',
        'Structured investigation blocks for AI agents',
        'SQL over DuckDB and Parquet',
      ],
      publisher: { '@id': `${SITE.url}/#organization` },
    },
  ],
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className="h-full">
      <body className="min-h-full bg-white text-neutral-900 antialiased dark:bg-neutral-950 dark:text-neutral-100">
        <a
          href="#main"
          className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded focus:bg-neutral-900 focus:px-3 focus:py-2 focus:text-white dark:focus:bg-white dark:focus:text-neutral-900"
        >
          Skip to content
        </a>
        {children}
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{ __html: JSON.stringify(graph) }}
        />
      </body>
    </html>
  );
}
