import { createFromSource } from 'fumadocs-core/search/server';
import { source } from '@/lib/source';

/** fumadocs' own search index — this is what ⌘K queries. */
export const { GET } = createFromSource(source);
