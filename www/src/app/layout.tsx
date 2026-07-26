import type { Metadata } from 'next';
import { Instrument_Sans, JetBrains_Mono } from 'next/font/google';
import type { ReactNode } from 'react';
import { SITE } from '@/lib/site';
import './globals.css';

/**
 * The pairing polylane uses: Instrument Sans for everything read, JetBrains Mono
 * for everything executed. Both self-hosted by vinext — no request to Google.
 *
 * Do NOT set `preload: false` to buy back a performance point: under vinext that
 * silently stops self-hosting and loads the woff2 from fonts.gstatic.com, which
 * sends every visitor's IP to Google. The score it appears to win is just the
 * third party dropping off the measured critical path.
 */
const sans = Instrument_Sans({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  display: 'swap',
  variable: '--font-instrument-sans',
});

const mono = JetBrains_Mono({
  subsets: ['latin'],
  weight: '400',
  display: 'swap',
  variable: '--font-jetbrains-mono',
});

export const metadata: Metadata = {
  metadataBase: new URL(SITE.url),
  title: { default: SITE.title, template: `%s — ${SITE.name}` },
  description: SITE.description,
  applicationName: SITE.name,
  // No canonical here: root-layout metadata is inherited, so a canonical set at
  // this level makes every /docs page claim the homepage as its canonical and
  // drop out of the index. Each page sets its own.
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

/** One @graph, cross-referenced, so an answer engine reads one entity not three. */
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
        'Structured investigation responses for AI agents',
        'SQL questions with no proprietary query language',
        'Machine-readable API description at /openapi.json',
      ],
      publisher: { '@id': `${SITE.url}/#organization` },
    },
  ],
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={`h-full ${sans.variable} ${mono.variable}`}>
      <body className="min-h-full bg-bg font-sans text-body antialiased">
        <a
          href="#main"
          className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded focus:bg-strong focus:px-3 focus:py-2 focus:text-on-accent"
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
