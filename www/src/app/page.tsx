import type { Metadata } from 'next';
import Link from 'next/link';
import { Faq } from '@/components/faq';
import { Install } from '@/components/install';
import { AnimatedSpan, Terminal, TypingAnimation } from '@/components/magicui/terminal';
import { FACTS, FAQ, LOOP, NAV, SITE, SOCIAL } from '@/lib/site';

export const metadata: Metadata = {
  alternates: { canonical: '/' },
};

// Nothing on this page is request-dependent — prerender it.
export const dynamic = 'force-static';

/** FAQPage generated from the same array the page renders. */
const faqSchema = {
  '@context': 'https://schema.org',
  '@type': 'FAQPage',
  '@id': `${SITE.url}/#faq`,
  isPartOf: { '@id': `${SITE.url}/#website` },
  mainEntity: FAQ.map((item) => ({
    '@type': 'Question',
    name: item.q,
    acceptedAnswer: { '@type': 'Answer', text: item.a },
  })),
};

const ANOMALIES = `$ sigiro anomalies
[3]{incident_id,service_name,operation,signal,ts_shift,before,after}:
  1,checkout,POST /api/orders,error_rate,2026-07-24T18:35:00Z,0.004,0.171
  1,checkout,POST /api/orders,latency_p95,2026-07-24T18:35:00Z,412.0,3180.0
  2,worker,worker,error_log_rate,2026-07-24T19:05:00Z,0.011,0.204`.split('\n');

const INVESTIGATE = `$ curl -XPOST localhost:9999/v1/investigate -d '{"service":"checkout"}'

{ "error_traces_total": 1284,
  "findings": [{
    "severity": "critical",
    "kind": "incident",
    "summary": "2 correlated shifts on checkout at 18:35",
    "incident": {
      "shifts": [
        { "signal": "error_rate",  "summary": "0.4% -> 17.1% at 18:35",
          "drill_down_sql": "SELECT * FROM sigiro_spans WHERE ..." },
        { "signal": "latency_p95", "summary": "412ms -> 3.18s at 18:35" }],
      "deploy": { "summary": "version 9f2c1ab at 18:33" } }}],
  "operation_summaries": [{
    "operation": "POST /api/orders", "error_pct": 17.1, "baseline_error_pct": 0.4,
    "error_breakdown": [{ "error_type": "timeout", "count": 246 },
                        { "error_type": "500", "count": 6 }],
    "drill_down_sql": "SELECT * FROM sigiro_spans WHERE ..." }],
  "gen_ai": [{ "model": "minimax-m2.7", "calls": 118, "p95_ms": 154000 }],
  "spans": { "total": 1284, "sampled_by": "errors first, then slowest",
             "drill_all_sql": "SELECT ..." }}`.split('\n');

const STEPS = [
  ['Run it', 'One container. Nothing else to install.'],
  ['Point OTLP at it', 'Any OpenTelemetry SDK you already have.'],
  ['Ask what changed', '`sigiro anomalies`, then `investigate`.'],
] as const;

export default function Home() {
  return (
    <>
      <header className="mx-auto flex max-w-4xl items-center justify-between px-6 py-6">
        <span className="font-mono text-lg font-semibold tracking-tight">{SITE.name}</span>
        <nav aria-label="Main" className="flex gap-5 text-sm">
          {NAV.map((item) => (
            <Link key={item.href} href={item.href} prefetch={false} className="hover:underline">
              {item.label}
            </Link>
          ))}
        </nav>
      </header>

      <main id="main" className="mx-auto max-w-4xl px-6 pb-32">
        {/* Hero: text LCP — no image, no motion, no web font. */}
        <section className="py-20 sm:py-28">
          <h1 className="max-w-2xl text-4xl font-semibold tracking-tight sm:text-6xl">
            {SITE.tagline}
          </h1>
          <p className="mt-6 max-w-xl text-lg text-neutral-600 dark:text-neutral-400">
            Observability your agent can act on. OpenTelemetry in, ranked findings out — each one
            carrying the SQL to dig deeper.
          </p>
          <div className="mt-10 max-w-xl">
            <Install />
          </div>
        </section>

        <section
          aria-labelledby="start"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <h2 id="start" className="text-sm font-mono text-neutral-500 dark:text-neutral-400">
            Start in 30 seconds
          </h2>
          <ol className="mt-6 grid gap-8 sm:grid-cols-3">
            {STEPS.map(([title, body], i) => (
              <li key={title}>
                <span className="font-mono text-xs text-neutral-500 dark:text-neutral-400">{i + 1}</span>
                <h3 className="mt-1 font-medium">{title}</h3>
                <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-400">{body}</p>
              </li>
            ))}
          </ol>
        </section>

        <section
          aria-labelledby="detect"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="detect" className="text-2xl font-semibold tracking-tight sm:text-3xl">
              Detection with nothing to configure
            </h2>
            <p className="mt-4 max-w-xl text-neutral-600 dark:text-neutral-400">
              No thresholds. sigiro watches each service against its own history and reports only
              what got worse. Shifts that move together become one incident.
            </p>
          </div>
          <div className="mt-8">
            <Terminal className="terminal max-h-none max-w-none">
              <TypingAnimation>{ANOMALIES[0]}</TypingAnimation>
              {ANOMALIES.slice(1).map((line, i) => (
                <AnimatedSpan
                  key={`${i}-${line}`}
                  className="whitespace-pre text-neutral-600 dark:text-neutral-400"
                >
                  {line}
                </AnimatedSpan>
              ))}
            </Terminal>
            <p className="mt-3 text-sm text-neutral-500 dark:text-neutral-400">
              TOON output — around 40% fewer tokens than JSON for this shape of data.
            </p>
          </div>
        </section>

        <section
          aria-labelledby="diagnose"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="diagnose" className="text-2xl font-semibold tracking-tight sm:text-3xl">
              One block of evidence, not a pile of spans
            </h2>
            <p className="mt-4 max-w-xl text-neutral-600 dark:text-neutral-400">
              An incident does not fit in a context window. One call returns what correlated, the
              suspect deploy, how each operation fails, and the query behind every row.
            </p>
          </div>
          <div className="mt-8">
            <Terminal className="terminal max-h-none max-w-none">
              <TypingAnimation>{INVESTIGATE[0]}</TypingAnimation>
              {INVESTIGATE.slice(1).map((line, i) => (
                <AnimatedSpan
                  key={`${i}-${line}`}
                  className="whitespace-pre text-neutral-600 dark:text-neutral-400"
                >
                  {line}
                </AnimatedSpan>
              ))}
            </Terminal>
            <p className="mt-3 text-sm text-neutral-500 dark:text-neutral-400">
              One call, trimmed. Every row carries the SQL that produced it.
            </p>
          </div>
        </section>

        <section
          aria-labelledby="loop"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="loop" className="text-2xl font-semibold tracking-tight sm:text-3xl">
              You need all five steps
            </h2>
            <p className="mt-4 max-w-xl text-neutral-600 dark:text-neutral-400">
              sigiro is two of them, and says so. Anything claiming all five is selling you a coding
              agent it did not write.
            </p>
          </div>
          <ol className="mt-8 divide-y divide-neutral-200 dark:divide-neutral-800">
            {LOOP.map((item) => (
              <li key={item.step} className="grid grid-cols-[7rem_1fr] items-baseline gap-4 py-3">
                <h3
                  className={
                    item.ours ? 'font-medium' : 'font-medium text-neutral-500 dark:text-neutral-400'
                  }
                >
                  {item.step}
                </h3>
                <p className="text-sm text-neutral-600 dark:text-neutral-400">
                  {item.body}{' '}
                  <span className="font-mono text-xs text-neutral-500 dark:text-neutral-400">— {item.owner}</span>
                </p>
              </li>
            ))}
          </ol>
        </section>

        <section
          aria-labelledby="facts"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <h2 id="facts" className="sr-only">
            How it fits
          </h2>
          <dl className="grid gap-8 sm:grid-cols-2">
            {FACTS.map(([term, def]) => (
              <div key={term}>
                <dt className="font-medium">{term}</dt>
                <dd className="mt-1 text-sm text-neutral-600 dark:text-neutral-400">{def}</dd>
              </div>
            ))}
          </dl>
        </section>

        <section
          aria-labelledby="faq"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <h2 id="faq" className="mb-6 text-2xl font-semibold tracking-tight sm:text-3xl">
            Questions
          </h2>
          <Faq />
        </section>
      </main>

      <footer className="border-t border-neutral-200 dark:border-neutral-800">
        <div className="mx-auto flex max-w-4xl flex-wrap items-center justify-between gap-4 px-6 py-8 text-sm text-neutral-500 dark:text-neutral-400">
          <span className="font-mono">{SITE.name}</span>
          <nav aria-label="Footer" className="flex flex-wrap gap-5">
            {NAV.map((item) => (
              <Link key={item.href} href={item.href} prefetch={false} className="hover:underline">
                {item.label}
              </Link>
            ))}
            {SOCIAL.map((item) => (
              <a key={item.href} href={item.href} rel="me noopener" className="hover:underline">
                {item.label}
              </a>
            ))}
            <a href="/llms.txt" className="hover:underline">
              llms.txt
            </a>
          </nav>
        </div>
      </footer>

      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(faqSchema) }}
      />
    </>
  );
}
