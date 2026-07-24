'use client';

import { Tabs } from '@base-ui-components/react/tabs';
import { useState } from 'react';
import { SITE } from '@/lib/site';

const HOSTED = `OTEL_EXPORTER_OTLP_ENDPOINT=https://api.sigiro.com \\
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $SIGIRO_API_KEY" \\
  <your-app-start-command>`;

function Command({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);

  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-b-lg border border-t-0 border-neutral-200 bg-neutral-50 p-4 pr-24 text-sm leading-relaxed dark:border-neutral-800 dark:bg-neutral-900">
        <code className="font-mono">{code}</code>
      </pre>
      <button
        type="button"
        className="absolute top-3 right-3 rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:bg-neutral-800 dark:hover:bg-neutral-700"
        onClick={() => {
          void navigator.clipboard.writeText(code).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
      <span aria-live="polite" className="sr-only">
        {copied ? 'Command copied to clipboard' : ''}
      </span>
    </div>
  );
}

export function Install() {
  return (
    <Tabs.Root defaultValue="self-host" className="w-full">
      <Tabs.List className="flex gap-1 rounded-t-lg border border-neutral-200 bg-neutral-100 p-1 dark:border-neutral-800 dark:bg-neutral-800/50">
        <Tabs.Tab
          value="self-host"
          className="rounded px-3 py-1.5 text-sm font-medium data-[selected]:bg-white data-[selected]:shadow-sm dark:data-[selected]:bg-neutral-900"
        >
          Self-host
        </Tabs.Tab>
        <Tabs.Tab
          value="hosted"
          className="rounded px-3 py-1.5 text-sm font-medium data-[selected]:bg-white data-[selected]:shadow-sm dark:data-[selected]:bg-neutral-900"
        >
          Hosted beta
        </Tabs.Tab>
      </Tabs.List>
      {/* keepMounted: both commands stay in the static HTML, so a crawler and an
          agent reading the page see the hosted path too. */}
      <Tabs.Panel value="self-host" keepMounted>
        <Command code={SITE.install} />
      </Tabs.Panel>
      <Tabs.Panel value="hosted" keepMounted>
        <Command code={HOSTED} />
      </Tabs.Panel>
    </Tabs.Root>
  );
}
