import { DocPage } from '@/components/doc-page';
import { getDoc } from '@/lib/docs';

const doc = getDoc('quickstart')!;

export const dynamic = 'force-static';

export const metadata = {
  title: doc.title,
  description: doc.description,
  alternates: { canonical: doc.url },
};

export default function Page() {
  return <DocPage doc={doc} />;
}
