import type { MetadataRoute } from 'next';
import { DOCS } from '@/lib/docs';
import { SITE } from '@/lib/site';

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: SITE.url, changeFrequency: 'weekly', priority: 1 },
    ...DOCS.map((doc) => ({
      url: `${SITE.url}${doc.url}`,
      changeFrequency: 'monthly' as const,
      priority: 0.7,
    })),
  ];
}
