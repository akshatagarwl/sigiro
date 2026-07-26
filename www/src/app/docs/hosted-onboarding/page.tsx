import { DocRoute } from '@/components/doc-route';
import { source } from '@/lib/source';

const page = source.getPage(['hosted-onboarding'])!;

export const dynamic = 'force-static';

export const metadata = {
  title: page.data.title,
  description: page.data.description,
  alternates: { canonical: page.url },
};

export default function Page() {
  return <DocRoute page={page} />;
}
