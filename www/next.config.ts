import type { NextConfig } from 'next';

/**
 * There is no MDX in this app any more — the docs are Blume at docs.sigiro.com.
 * This file carries headers and the published redirects, and nothing else. It is
 * still read by the `build:next` fallback.
 */
const nextConfig: NextConfig = {
  reactStrictMode: true,
  // 17 KB of CSS was the only render-blocking request left; inlining it removes
  // a round trip on first paint.
  experimental: { inlineCss: true },
  /**
   * The brand short links. These are published — in the README, in the install
   * instructions, on the socials — so they are part of the deployed contract and
   * not a convenience.
   *
   * They were `_redirects` on the Cloudflare Pages site that used to serve the
   * apex, and only two of the six had been carried over here. Enumerated off the
   * live Pages site before the apex moved to this Worker; do not drop one without
   * checking where it is published first.
   *
   * `permanent: false` throughout: a 301 is cached by the browser more or less
   * forever, and every one of these points at a third-party account that can move.
   * A social handle is not a canonical URL change.
   */
  /**
   * Documents revalidate; fingerprinted assets do not.
   *
   * Static routes were going out as `s-maxage=31536000, stale-while-revalidate`,
   * which tells every shared cache — Cloudflare's edge, and whatever proxy sits
   * in front of a link-preview crawler — that this HTML is good for a year. It
   * is a marketing page whose whole job is to describe a product that changes,
   * and it carries the og:title, og:description and og:image every scraper
   * reads. A year-old copy of that is a year-old preview.
   *
   * Five minutes at the edge, then revalidate, with a day of
   * stale-while-revalidate so a reader never waits on the origin. `max-age=0`
   * keeps browsers honest without touching the shared cache.
   *
   * `/_next/static/*` is content-hashed, so a new build is a new URL and there
   * is nothing to go stale. It keeps the year, immutably.
   */
  async headers() {
    return [
      {
        source: '/:path*',
        headers: [
          {
            key: 'Cache-Control',
            value: 'public, max-age=0, s-maxage=300, stale-while-revalidate=86400',
          },
        ],
      },
      {
        source: '/_next/static/:path*',
        headers: [{ key: 'Cache-Control', value: 'public, max-age=31536000, immutable' }],
      },
    ];
  },
  async redirects() {
    return [
      { source: '/github', destination: 'https://github.com/sigiroai', permanent: false },
      { source: '/x', destination: 'https://x.com/SigiroAI', permanent: false },
      { source: '/twitter', destination: 'https://x.com/SigiroAI', permanent: false },
      {
        source: '/linkedin',
        destination: 'https://www.linkedin.com/company/sigiro',
        permanent: false,
      },
      { source: '/discord', destination: 'https://discord.gg/sigiro', permanent: false },
      /**
       * The docs moved to Blume at docs.sigiro.com, where they sit at the root
       * rather than under `/docs`. Every one of these paths was published — in
       * the README, in the nav, in the landing copy, and in the docs' own
       * cross-links — so they redirect rather than 404.
       *
       * `permanent: false` like the rest of this list. A 301 is cached by the
       * browser more or less forever, and the docs have just moved once already.
       *
       * `/docs/api/*` has no per-operation equivalent: the reference is now one
       * generated page from the live spec, so every operation lands on it.
       */
      { source: '/docs', destination: 'https://docs.sigiro.com', permanent: false },
      { source: '/docs/api/:path*', destination: 'https://docs.sigiro.com/api', permanent: false },
      { source: '/docs/:path*', destination: 'https://docs.sigiro.com/:path*', permanent: false },
      /**
       * Blume generates these from the same content, so they live beside it.
       * The paths stay reachable here because the footer, the README and the
       * llms.txt convention itself all point at the site root.
       */
      { source: '/llms.txt', destination: 'https://docs.sigiro.com/llms.txt', permanent: false },
      {
        source: '/llms-full.txt',
        destination: 'https://docs.sigiro.com/llms-full.txt',
        permanent: false,
      },
      /**
       * `/install` is no longer here. It was a 307 to
       * `sigiroai/sigiro/releases/latest/download/install.sh`, and that release
       * — and that repository — do not exist, so the one short link that is
       * printed in the README and on the socials answered 404 to everyone who
       * ran it. It is now a real script at `public/install`, served as a static
       * asset. A redirect would also have broken `curl -fsS` without `-L`.
       */
    ];
  },
};

export default nextConfig;
