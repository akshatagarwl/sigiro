import type { Metadata } from 'next';
import Link from 'next/link';
import { Faq } from '@/components/faq';
import { AgentLoop } from '@/components/agent-loop';
import { Changepoint } from '@/components/changepoint';
import { DashboardVsEvidence } from '@/components/dashboard-vs-evidence';
import { Install } from '@/components/install';
import { LoopDiagram } from '@/components/loop-diagram';
import { AnimatedSpan, Terminal, TypingAnimation } from '@/components/magicui/terminal';
import { FAQ, NAV, SITE, SOCIAL } from '@/lib/site';

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

const SESSION = `$ sigiro anomalies
{ "anomalies": [
    { "service_name": "checkout", "operation": "POST /api/orders",
      "signal": "error_rate", "kind": "changepoint", "direction": "worsening",
      "ts_shift": "2026-07-24T18:35:00Z", "incident_id": "1",
      "before": 0.4, "after": 17.1,
      "drill_down_sql": "SELECT * FROM sigiro_spans WHERE ..." },
    { "service_name": "checkout", "operation": "POST /api/orders",
      "signal": "latency_p95", "incident_id": "1",
      "before": 412000, "after": 3180000 },
    { "service_name": "worker", "operation": "worker",
      "signal": "error_log_rate", "incident_id": null,
      "before": 1.1, "after": 20.4 }]}

$ sigiro investigate checkout
{ "error_traces_total": 1284,
  "findings": [{ "severity": "critical", "kind": "incident",
    "summary": "2 correlated shifts on checkout at 18:35",
    "incident": { "shifts": [
      { "signal": "error_rate",  "summary": "0.4% -> 17.1% at 18:35" },
      { "signal": "latency_p95", "summary": "412ms -> 3.18s at 18:35" }],
      "deploy": { "summary": "version 9f2c1ab at 18:33" } }}],
  "operation_summaries": [{ "operation": "POST /api/orders",
    "error_pct": 17.1, "baseline_error_pct": 0.4,
    "error_breakdown": [{ "error_type": "timeout", "count": 246 }],
    "drill_down_sql": "SELECT * FROM sigiro_spans WHERE ..." }],
  "gen_ai": [{ "model": "minimax-m2.7", "calls": 118, "p95_ms": 154000 }]}`.split('\n');

export default function Home() {
  return (
    <>
      <header className="mx-auto flex max-w-3xl items-center justify-between px-6 py-6">
        <span className="flex items-center gap-2 font-mono text-lg tracking-tight">
          <svg
              viewBox="0 0 32 32"
              aria-hidden
              className="size-6 shrink-0"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.75"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M5 24h7V10h15" />
              <circle cx="12" cy="10" r="3.25" fill="var(--color-background)" />
            </svg>
          {SITE.name}
        </span>
        <nav aria-label="Main" className="flex gap-5 text-sm">
          {NAV.map((item) => (
            <Link key={item.href} href={item.href} prefetch={false} className="hover:underline">
              {item.label}
            </Link>
          ))}
        </nav>
      </header>

      <main id="main" className="mx-auto max-w-3xl px-6 pb-20">
        {/* Hero: text LCP — no image, no motion, no web font. */}
        <section className="py-14 sm:py-20">
          <h1 className="max-w-2xl text-3xl font-semibold tracking-tight sm:text-5xl">
            {SITE.tagline}
          </h1>
          <p className="mt-5 max-w-xl text-neutral-600 dark:text-neutral-400">
            Observability your agent can act on. OpenTelemetry in, ranked findings out — each one
            carrying the SQL to dig deeper.
          </p>
          <div className="mt-8 max-w-xl">
            <Install />
          </div>
        </section>

        <section
          aria-labelledby="evidence"
          className="border-t border-neutral-200 py-12 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="evidence" className="text-xl font-semibold tracking-tight sm:text-2xl">
              Ask what changed, then why
            </h2>
            <p className="mt-4 max-w-xl text-neutral-600 dark:text-neutral-400">
              No thresholds to configure: each service is watched against its own history, and
              shifts that move together share an incident id. One more call returns the evidence —
              the suspect deploy, how the operation fails, the query behind every row. Every
              command answers in JSON, so the reader can be a person or a process. Values are raw
              signal units — error rate in percent, p95 in microseconds.
            </p>
          </div>
          <div className="mt-8">
            <Changepoint />
          </div>
          <div className="mt-8">
            <Terminal sequence={false} className="terminal max-h-none max-w-none">
              <TypingAnimation>{SESSION[0]}</TypingAnimation>
              {SESSION.slice(1).map((line, i) => (
                <AnimatedSpan
                  key={`${i}-${line}`}
                  className="whitespace-pre text-neutral-700 dark:text-neutral-300"
                >
                  {line}
                </AnimatedSpan>
              ))}
            </Terminal>
          </div>
        </section>

        <section
          aria-labelledby="loop"
          className="border-t border-neutral-200 py-12 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="loop" className="text-xl font-semibold tracking-tight sm:text-2xl">
              You need all five steps
            </h2>
            <p className="mt-4 max-w-xl text-neutral-600 dark:text-neutral-400">
              sigiro is two of them, and says so. Anything claiming all five is selling you a coding
              agent it did not write.
            </p>
          </div>
          <LoopDiagram />
        </section>

        <section
          aria-labelledby="contrast"
          className="border-t border-neutral-200 py-12 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="contrast" className="text-xl font-semibold tracking-tight sm:text-2xl">
              A chart is for you. This is for the agent.
            </h2>
          </div>
          <div className="mt-8">
            <DashboardVsEvidence />
          </div>
        </section>

        <section
          aria-labelledby="agent"
          className="border-t border-neutral-200 py-12 dark:border-neutral-800"
        >
          <div className="reveal">
            <h2 id="agent" className="text-xl font-semibold tracking-tight sm:text-2xl">
              Six calls, symptom to pull request
            </h2>
            <p className="mt-4 max-w-xl text-neutral-600 dark:text-neutral-400">
              Nothing here is a person reading a dashboard. sigiro answers the first five; the sixth
              is your coding agent, which is the point.
            </p>
          </div>
          <div className="mt-8 max-w-2xl">
            <AgentLoop />
          </div>
        </section>

        <section
          aria-labelledby="faq"
          className="border-t border-neutral-200 py-12 dark:border-neutral-800"
        >
          <h2 id="faq" className="mb-4 text-xl font-semibold tracking-tight sm:text-2xl">
            Questions
          </h2>
          <Faq />
        </section>
      </main>

      <footer className="border-t border-neutral-200 dark:border-neutral-800">
        <div className="mx-auto flex max-w-3xl flex-wrap items-center justify-between gap-4 px-6 py-8 text-sm text-neutral-500 dark:text-neutral-400">
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
