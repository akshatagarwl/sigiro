import type { MetadataRoute } from 'next';
import { source } from '@/app/source';
import { SITE } from '@/lib/site';

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: SITE.url, changeFrequency: 'weekly', priority: 1 },
    ...source.getPages().map((page) => ({
      url: `${SITE.url}${page.url}`,
      changeFrequency: 'monthly' as const,
      priority: 0.7,
    })),
  ];
}
