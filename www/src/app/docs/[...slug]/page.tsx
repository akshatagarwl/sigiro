import { notFound } from 'next/navigation';
import { DocRoute } from '@/components/doc-route';
import { source } from '@/lib/source';

/**
 * Everything except the four hand-written docs, which have static route files so
 * their metadata reaches <head>. Rendering lives in DocRoute — this file only
 * resolves the page.
 */
export const dynamic = 'force-static';

export function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata({ params }: { params: Promise<{ slug?: string[] }> }) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  return {
    title: page.data.title,
    description: page.data.description,
    alternates: { canonical: page.url },
  };
}

export default async function Page({ params }: { params: Promise<{ slug?: string[] }> }) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  return <DocRoute page={page} />;
}
