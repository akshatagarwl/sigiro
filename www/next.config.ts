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
  async redirects() {
    return [
      { source: '/x', destination: 'https://x.com/SigiroAI', permanent: false },
      {
        source: '/linkedin',
        destination: 'https://www.linkedin.com/company/sigiro',
        permanent: false,
      },
    ];
  },
};

export default nextConfig;
