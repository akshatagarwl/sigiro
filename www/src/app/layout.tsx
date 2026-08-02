import type { Metadata, Viewport } from 'next';
import { Geist, Geist_Mono } from 'next/font/google';
import { ThemeProvider } from 'next-themes';
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

/**
 * Theme lives in `next-themes`, which was already in the tree — fumadocs-ui
 * depends on it — and is now a direct dependency because we import it.
 *
 * It replaced a hand-written blocking script here and a small state machine in
 * `theme-toggle.tsx`. Both were correct. Neither did the two things this does
 * for nothing: follow the operating system when it changes theme *while the
 * page is open* (a `matchMedia` listener, which the old script could not have —
 * it ran once before paint and never again), and follow the same choice made in
 * another tab (a `storage` listener).
 *
 * `storageKey` keeps the key readers already have, so nobody's saved preference
 * is dropped by this change. `attribute="class"` writes the same `.dark` on
 * `<html>` that globals.css already keys off, and it injects its own pre-paint
 * script, so there is still no flash of the wrong material.
 *
 * `defaultTheme="system"` with `enableSystem` reproduces the old rule exactly:
 * a stored choice wins, and with no stored choice `prefers-color-scheme`
 * decides.
 *
 * The docs layout's `RootProvider` keeps `theme={{ enabled: false }}`. fumadocs
 * would otherwise mount a second provider for the same class.
 */

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
      <body className="flex min-h-full flex-col font-sans text-ink-2 antialiased">
        <ThemeProvider
          attribute="class"
          storageKey="sigiro-theme"
          defaultTheme="system"
          enableSystem
          disableTransitionOnChange
        >
          <a
            href="#main"
            className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-[9px] focus:bg-ink focus:px-3 focus:py-2 focus:text-panel"
          >
            Skip to content
          </a>
          {children}
        </ThemeProvider>
        <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: jsonLd(graph) }} />
      </body>
    </html>
  );
}
