import { DocRoute } from '@/components/doc-route';
import { source } from '@/lib/source';

/**
 * The reference index gets a static route for the same reason the four
 * hand-written docs do: a static `metadata` export lands in <head>, where an
 * async `generateMetadata` on the catch-all does not. This is the page the header
 * links to, so it is the one that has to be indexable on its own terms; the five
 * operation pages stay on the catch-all and inherit the layout's description.
 */
const page = source.getPage(['api'])!;

export const dynamic = 'force-static';

export const metadata = {
  title: page.data.title,
  description: page.data.description,
  alternates: { canonical: page.url },
};

export default function Page() {
  return <DocRoute page={page} />;
}
