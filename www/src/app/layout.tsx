import type { Metadata } from 'next';
import { JetBrains_Mono } from 'next/font/google';
import type { ReactNode } from 'react';
import { SITE } from '@/lib/site';
import './globals.css';

/**
 * The only web font on the site. The body stays on the system sans stack — a
 * product whose surface is a terminal and a SQL string earns one real mono, and
 * nothing else. One static 400 cut (22 KB) rather than the
 * variable font (40 KB, every weight axis, and nothing here needs a second
 * weight). vinext self-hosts it — no request to Google.
 */
const mono = JetBrains_Mono({
  subsets: ['latin'],
  weight: '400',
  display: 'swap',
  // Not preloaded. The LCP element is the h1 in system sans; the mono is only
  // the wordmark, the terminal and code. Preloading put 22 KB on the critical
  // path and cost 14 points of mobile performance (LCP 1.1s -> 3.2s) for text
  // that is not above the fold. next/font's fallback metrics hold CLS at 0
  // through the swap.
  preload: false,
  variable: '--font-jetbrains-mono',
});

export const metadata: Metadata = {
  metadataBase: new URL(SITE.url),
  title: {
    default: SITE.title,
    template: `%s — ${SITE.name}`,
  },
  description: SITE.description,
  applicationName: SITE.name,
  // No `alternates.canonical` here on purpose: root-layout metadata is inherited,
  // so a canonical set here makes every /docs page claim the homepage as its
  // canonical and drops them out of the index. Each page sets its own.
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
    <html lang="en" className={`h-full ${mono.variable}`}>
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
