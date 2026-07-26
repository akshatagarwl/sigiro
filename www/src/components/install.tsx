'use client';

import {
  Snippet,
  SnippetCopyButton,
  SnippetHeader,
  SnippetTabsContent,
  SnippetTabsList,
  SnippetTabsTrigger,
} from '@/components/kibo-ui/snippet';
import { INSTALL } from '@/lib/site';

/**
 * Two ways in, and the clipboard behaviour comes from @kibo-ui/snippet rather
 * than hand-rolled — it owns the copy state and the icon swap.
 */
export function Install() {
  return (
    <Snippet defaultValue={INSTALL[0].id} className="mt-8 w-full max-w-2xl">
      <SnippetHeader>
        <SnippetTabsList>
          {INSTALL.map((item) => (
            <SnippetTabsTrigger key={item.id} value={item.id}>
              {item.label}
            </SnippetTabsTrigger>
          ))}
        </SnippetTabsList>
        <SnippetCopyButton value={INSTALL[0].code} aria-label="Copy command" />
      </SnippetHeader>
      {INSTALL.map((item) => (
        <SnippetTabsContent key={item.id} value={item.id}>
          {item.code}
        </SnippetTabsContent>
      ))}
    </Snippet>
  );
}
