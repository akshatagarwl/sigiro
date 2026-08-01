import type { Metadata, Viewport } from 'next';
import { Geist, Geist_Mono } from 'next/font/google';
import type { ReactNode } from 'react';
import { SITE } from '@/lib/site';
import './globals.css';
import { jsonLd } from '@/lib/json-ld';

/**
 * The pairing the design language is written for: Geist for everything read,
 * Geist Mono for everything executed. Both self-hosted by vinext — no request to
 * Google.
 *
 * Do NOT set `preload: false` to buy back a performance point: under vinext that
 * silently stops self-hosting and loads the woff2 from fonts.gstatic.com, which
 * sends every visitor's IP to Google. The score it appears to win is just the
 * third party dropping off the measured critical path.
 */
const sans = Geist({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  display: 'swap',
  variable: '--font-geist-sans',
});

const mono = Geist_Mono({
  subsets: ['latin'],
  weight: '400',
  display: 'swap',
  variable: '--font-geist-mono',
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
    images: [{ url: SITE.ogImage, width: 1200, height: 630, alt: SITE.tagline, type: 'image/png' }],
  },
  twitter: {
    card: 'summary_large_image',
    site: '@SigiroAI',
    title: SITE.title,
    description: SITE.description,
    images: [SITE.ogImage],
  },
  robots: { index: true, follow: true },
};

export const viewport: Viewport = {
  themeColor: [
    { media: '(prefers-color-scheme: light)', color: '#efeeea' },
    { media: '(prefers-color-scheme: dark)', color: '#141312' },
  ],
};

/** Sets the theme class before first paint, so there is no flash of the wrong
 *  material. Inline and blocking on purpose — anything async is a flash. */
const themeScript = `(function(){try{var s=localStorage.getItem("sigiro-theme");var d=s?s==="dark":matchMedia("(prefers-color-scheme:dark)").matches;if(d)document.documentElement.classList.add("dark")}catch(e){}})()`;

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
    <html lang="en" suppressHydrationWarning className={`h-full ${sans.variable} ${mono.variable}`}>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript }} />
      </head>
      <body className="flex min-h-full flex-col font-sans text-ink-2 antialiased">
        <a
          href="#main"
          className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-[9px] focus:bg-ink focus:px-3 focus:py-2 focus:text-panel"
        >
          Skip to content
        </a>
        {children}
        <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: jsonLd(graph) }} />
      </body>
    </html>
  );
}
