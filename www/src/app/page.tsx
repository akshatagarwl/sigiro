import type { Metadata } from 'next';
import Link from 'next/link';
import { Faq } from '@/components/faq';
import { Install } from '@/components/install';
import { Reveal } from '@/components/reveal';
import { FAQ, LOOP, NAV, SITE, SOCIAL } from '@/lib/site';

export const metadata: Metadata = {
  alternates: { canonical: '/' },
};

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
  2,worker,worker,error_log_rate,2026-07-24T19:05:00Z,0.011,0.204`;

const INVESTIGATE = `POST /v1/investigate  {"service": "checkout"}

{
  "error_traces_total": 1284,
  "findings": [
    {
      "severity": "critical",
      "kind": "incident",
      "summary": "2 correlated shifts on checkout at 18:35",
      "incident": {
        "started_at_iso": "2026-07-24T18:35:00Z",
        "shifts": [
          { "signal": "error_rate",  "entity": "operation 'POST /api/orders'",
            "summary": "error rate shifted at 18:35 (0.4% -> 17.1%)",
            "drill_down_sql": "SELECT * FROM spans WHERE ..." },
          { "signal": "latency_p95", "entity": "operation 'POST /api/orders'",
            "summary": "p95 shifted at 18:35 (412ms -> 3.18s)" }
        ],
        "deploy": { "kind": "deploy", "summary": "version 9f2c1ab at 18:33" }
      }
    }
  ],
  "operation_summaries": [
    { "operation": "POST /api/orders", "count": 7421, "error_pct": 17.1,
      "baseline_error_pct": 0.4, "is_anomalous": true,
      "error_breakdown": [ { "error_type": "timeout", "count": 246 },
                           { "error_type": "500", "count": 6 } ],
      "drill_down_sql": "SELECT * FROM spans WHERE ..." }
  ],
  "gen_ai": [
    { "system": "nvidia", "model": "minimax-m2.7", "calls": 118,
      "error_pct": 9.3, "p95_ms": 154000, "output_tokens": 389412 }
  ],
  "spans": { "total": 1284, "sampled_by": "errors first, then slowest",
             "sample": [ /* 25 rows */ ], "drill_all_sql": "SELECT ..." }
}`;

function Terminal({ children, label }: { children: string; label: string }) {
  return (
    <figure className="overflow-hidden rounded-lg border border-neutral-200 dark:border-neutral-800">
      <figcaption className="border-b border-neutral-200 bg-neutral-100 px-4 py-2 font-mono text-xs text-neutral-600 dark:border-neutral-800 dark:bg-neutral-900 dark:text-neutral-400">
        {label}
      </figcaption>
      <pre className="overflow-x-auto bg-neutral-50 p-4 text-xs leading-relaxed dark:bg-neutral-950">
        <code className="font-mono">{children}</code>
      </pre>
    </figure>
  );
}

const FIT = [
  [
    'OpenTelemetry, natively',
    'OTLP on 4317 (gRPC) and 4318 (HTTP) for traces, logs, metrics and profiles, plus Pyroscope-format profiles. Point OTEL_EXPORTER_OTLP_ENDPOINT at it and nothing else changes.',
  ],
  [
    'One process, no sidecars',
    'A single container with DuckDB embedded. Data is Parquet on local disk or S3-compatible storage. No collector, no agent, no external database to operate.',
  ],
  [
    'SQL, not a DSL',
    'Your telemetry is tables. POST /v1/query takes SQL, the CLI takes SQL, and every finding hands you SQL. There is nothing new to learn and nothing to escape from.',
  ],
  [
    'Discoverable by machines',
    'GET /openapi.json describes the whole surface with no auth, so an agent can learn the API without a tool description written by hand.',
  ],
] as const;

export default function Home() {
  return (
    <>
      <header className="mx-auto flex max-w-5xl items-center justify-between px-6 py-5">
        <span className="font-mono text-lg font-semibold tracking-tight">{SITE.name}</span>
        <nav aria-label="Main" className="flex gap-5 text-sm">
          {NAV.map((item) => (
            <Link key={item.href} href={item.href} className="hover:underline">
              {item.label}
            </Link>
          ))}
        </nav>
      </header>

      <main id="main" className="mx-auto max-w-5xl px-6 pb-24">
        {/* Hero: text LCP — no image, no motion, no web font. */}
        <section className="py-16 sm:py-24">
          <p className="mb-4 font-mono text-sm text-neutral-500 dark:text-neutral-400">
            detect → diagnose → fix → verify → learn
          </p>
          <h1 className="max-w-3xl text-4xl font-semibold tracking-tight sm:text-6xl">
            {SITE.tagline}
          </h1>
          <p className="mt-6 max-w-2xl text-lg text-neutral-700 dark:text-neutral-300">
            You do not have a software factory until every step of the loop is covered. sigiro is
            the first two: it watches your OpenTelemetry for things that changed, then hands your
            agent one structured block of evidence explaining why — with the SQL to dig further
            attached to every row.
          </p>
          <div className="mt-10 max-w-2xl">
            <Install />
          </div>
          <p className="mt-4 text-sm text-neutral-600 dark:text-neutral-400">
            Three ports: OTLP gRPC on 4317, OTLP HTTP on 4318, the query and investigate API on
            9999. No collector to deploy, no configuration file.{' '}
            <Link href="/docs/quickstart" className="underline">
              Quickstart
            </Link>
            .
          </p>
        </section>

        <section
          aria-labelledby="loop"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <Reveal>
            <h2 id="loop" className="text-3xl font-semibold tracking-tight">
              A software factory is five steps, and you need all five
            </h2>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              Every step has to hand off to the next without a person in between. Cover four and the
              loop still stops at a human — you have made paging someone faster, not made the
              software self-operating. Here is the whole loop, and who owns each part.
            </p>
          </Reveal>
          <ol className="mt-10 space-y-6">
            {LOOP.map((item, i) => (
              <Reveal key={item.step} delay={i * 0.04}>
                <li className="grid gap-2 border-l-2 border-neutral-200 pl-5 sm:grid-cols-[10rem_1fr] sm:gap-6 dark:border-neutral-800">
                  <div>
                    <h3 className="text-lg font-medium">{item.step}</h3>
                    <p
                      className={
                        item.ours
                          ? 'font-mono text-xs text-neutral-900 dark:text-neutral-100'
                          : 'font-mono text-xs text-neutral-500 dark:text-neutral-400'
                      }
                    >
                      {item.owner}
                    </p>
                  </div>
                  <p className="text-neutral-700 dark:text-neutral-300">{item.body}</p>
                </li>
              </Reveal>
            ))}
          </ol>
          <p className="mt-8 max-w-2xl text-neutral-700 dark:text-neutral-300">
            sigiro deliberately does two of them. Anything claiming to do all five is selling you a
            coding agent it did not write and a pipeline it does not run.
          </p>
        </section>

        <section
          aria-labelledby="detect"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <Reveal>
            <h2 id="detect" className="text-3xl font-semibold tracking-tight">
              Detection you did not have to configure
            </h2>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              An agent cannot poll an alert rule nobody wrote yet. sigiro runs Bayesian changepoint
              detection over each service&apos;s own recent history — error rate, p95 latency, log
              volume, error log rate, profile cost — and keeps only the shifts that made things
              worse. No thresholds, no baselines to seed. A service that is chronically slow but
              stable is not an anomaly, because nothing changed.
            </p>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              Shifts landing in the same detection bucket on one service share an{' '}
              <code className="font-mono text-sm">incident_id</code>, so a latency spike and an
              error spike from one bad deploy arrive as one incident instead of two pages.
            </p>
          </Reveal>
          <div className="mt-8">
            <Terminal label="sigiro anomalies — TOON output, ~40% fewer tokens than JSON">
              {ANOMALIES}
            </Terminal>
          </div>
        </section>

        <section
          aria-labelledby="diagnose"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <Reveal>
            <h2 id="diagnose" className="text-3xl font-semibold tracking-tight">
              Diagnosis as one block of evidence, not a pile of spans
            </h2>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              An incident does not fit in a context window, so sigiro does not try to send you one.
              One call to <code className="font-mono text-sm">/v1/investigate</code> returns tens of
              kilobytes of ranked evidence for a service and window: what correlated, which deploy is
              the suspect, how each operation is failing broken down by error type, per-model LLM
              latency and token counts, deduplicated log patterns, and a sample of spans that states
              how it was sampled and how much it withheld.
            </p>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              Every row carries the SQL that produced it. That is the part that matters for an agent:
              it can widen, narrow or pivot any answer by editing a query it was handed, instead of
              guessing at a schema it has never seen.
            </p>
          </Reveal>
          <div className="mt-8">
            <Terminal label="POST /v1/investigate — trimmed">{INVESTIGATE}</Terminal>
          </div>
        </section>

        <section
          aria-labelledby="why"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <Reveal>
            <h2 id="why" className="text-3xl font-semibold tracking-tight">
              Why not point an agent at the dashboards you already have
            </h2>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              Because a dashboard is a human interface, and the API bolted to its side returns
              whatever the chart needed. An agent asking what is wrong with checkout gets back time
              series it has to interpret, in a query language it has to learn, with no ranking and no
              way to go deeper except guessing the next query.
            </p>
            <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
              sigiro inverts the priority. The primary interface returns ranked findings plus the
              exact SQL behind them, and the storage underneath is DuckDB over Parquet you can query
              directly — no proprietary layer between an agent and its own telemetry. If a person is
              reading, keep your dashboard. If an agent is reading, the shape of the answer matters
              more than the shape of the chart.
            </p>
          </Reveal>
        </section>

        <section
          aria-labelledby="fit"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <Reveal>
            <h2 id="fit" className="text-3xl font-semibold tracking-tight">
              It drops into what you already run
            </h2>
          </Reveal>
          <dl className="mt-8 grid gap-6 sm:grid-cols-2">
            {FIT.map(([term, def]) => (
              <Reveal key={term}>
                <div>
                  <dt className="font-medium">{term}</dt>
                  <dd className="mt-2 text-neutral-700 dark:text-neutral-300">{def}</dd>
                </div>
              </Reveal>
            ))}
          </dl>
        </section>

        <section
          aria-labelledby="faq"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <h2 id="faq" className="mb-8 text-3xl font-semibold tracking-tight">
            Questions
          </h2>
          <Faq />
        </section>

        <section
          aria-labelledby="start"
          className="border-t border-neutral-200 py-16 dark:border-neutral-800"
        >
          <h2 id="start" className="text-3xl font-semibold tracking-tight">
            Start with one container
          </h2>
          <p className="mt-4 max-w-2xl text-neutral-700 dark:text-neutral-300">
            Run it, point a service at it, and ask it what changed. If you would rather not run
            anything, the hosted service is an invited beta — the{' '}
            <Link href="/docs/hosted-onboarding" className="underline">
              onboarding guide
            </Link>{' '}
            explains how access works.
          </p>
          <div className="mt-8 max-w-2xl">
            <Install />
          </div>
        </section>
      </main>

      <footer className="border-t border-neutral-200 dark:border-neutral-800">
        <div className="mx-auto flex max-w-5xl flex-wrap items-center justify-between gap-4 px-6 py-8 text-sm text-neutral-600 dark:text-neutral-400">
          <span className="font-mono">{SITE.name}</span>
          <nav aria-label="Footer" className="flex flex-wrap gap-5">
            {NAV.map((item) => (
              <Link key={item.href} href={item.href} className="hover:underline">
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
