import type { NextConfig } from 'next';

/**
 * MDX comes from fumadocs-mdx/vite in vite.config.ts, not from createMDX here —
 * this file only carries config that is genuinely framework-level. It is still
 * read by the `build:next` fallback.
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
      {
        source: '/install',
        destination: 'https://github.com/sigiroai/sigiro/releases/latest/download/install.sh',
        permanent: false,
      },
    ];
  },
};

export default nextConfig;
