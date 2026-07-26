import type { MetadataRoute } from 'next';
import { SITE } from '@/lib/site';

/**
 * Everything is allowed — the AI crawlers are listed explicitly rather than
 * relying on the implicit default, so "are we opted in?" is answerable by
 * reading the file instead of reasoning about the absence of a rule.
 */
export default function robots(): MetadataRoute.Robots {
  return {
    rules: [
      { userAgent: '*', allow: '/' },
      {
        userAgent: [
          'GPTBot',
          'OAI-SearchBot',
          'ClaudeBot',
          'Claude-User',
          'PerplexityBot',
          'Google-Extended',
        ],
        allow: '/',
      },
    ],
    sitemap: `${SITE.url}/sitemap.xml`,
    host: SITE.url,
  };
}
