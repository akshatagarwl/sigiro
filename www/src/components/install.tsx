import { SITE } from '@/lib/site';

/**
 * Server component. The copy button is fifteen lines of delegated vanilla JS
 * instead of a client component — one `use client` anywhere on this page pulls in
 * the ~390 KB React client runtime, which cost 20 points of mobile performance.
 */
const COPY_SCRIPT = `document.addEventListener('click',function(e){
var b=e.target.closest('[data-copy]');if(!b)return;
navigator.clipboard.writeText(b.getAttribute('data-copy')).then(function(){
var p=b.textContent;b.textContent='Copied';setTimeout(function(){b.textContent=p},1500)})});`;

export function Install() {
  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-xl border border-neutral-200 bg-neutral-50 p-4 pr-24 text-sm dark:border-neutral-800 dark:bg-neutral-900">
        <code className="font-mono">{SITE.install}</code>
      </pre>
      <button
        type="button"
        data-copy={SITE.install}
        className="absolute top-3 right-3 rounded border border-neutral-300 bg-white px-2 py-1 text-xs font-medium hover:bg-neutral-100 dark:border-neutral-700 dark:bg-neutral-800 dark:hover:bg-neutral-700"
      >
        Copy
      </button>
      <script dangerouslySetInnerHTML={{ __html: COPY_SCRIPT }} />
    </div>
  );
}
