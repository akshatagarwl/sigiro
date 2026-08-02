import type { MetadataRoute } from 'next';
import { SITE } from '@/lib/site';

/**
 * One page, because this site is now one page.
 *
 * The docs used to be enumerated here from fumadocs' `source`. They live at
 * docs.sigiro.com and Blume generates their own sitemap, which is the right
 * place for them: a sitemap describes one origin, and another origin's URLs
 * listed here would be ignored.
 */
export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url: SITE.url, changeFrequency: 'weekly', priority: 1 }];
}
