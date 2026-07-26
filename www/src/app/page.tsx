import type { Metadata } from 'next';
import Link from 'next/link';
import { Artifact } from '@/components/artifact';
import { Faq } from '@/components/faq';
import { Install } from '@/components/install';
import { FAQ, NAV, PROMISES, RUNS_ON, SECTIONS, SITE, SOCIAL } from '@/lib/site';

export const metadata: Metadata = {
  alternates: { canonical: '/' },
};

// Nothing here is request-dependent, so it prerenders — which is also what lets
// Shiki colour every artifact at build time instead of in the browser.
export const dynamic = 'force-static';

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

const NUM = 'font-mono text-[0.6875rem] tracking-[0.2em] text-faint';
const H2 =
  'mt-2.5 max-w-[34rem] text-[1.375rem] leading-[1.35] font-semibold tracking-[-0.025em] text-strong';
const SAY = 'mt-2.5 max-w-[36rem] text-muted-ink';

/** The changepoint glyph: a flat baseline that steps up. */
function Mark() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 32 32"
      fill="none"
      stroke="currentColor"
      strokeWidth="3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M5 24h7V10h15" />
    </svg>
  );
}

export default function Home() {
  return (
    <>
      <div className="mx-auto max-w-[60rem] px-6">
        <header className="flex items-center justify-between py-5">
          <span className="flex items-center gap-2 font-semibold tracking-tight text-strong">
            <Mark />
            {SITE.name}
          </span>
          <nav aria-label="Main" className="flex items-center gap-5 text-[0.8125rem]">
            {NAV.map((item) => (
              <Link
                key={item.href}
                href={item.href}
                prefetch={false}
                className="text-muted-ink hover:text-strong"
              >
                {item.label}
              </Link>
            ))}
            <Link
              href="/docs/quickstart"
              prefetch={false}
              className="rounded-lg border border-line-strong px-3 py-1.5 font-medium text-strong"
            >
              Get started
            </Link>
          </nav>
        </header>

        <main id="main">
          {/* Hero: text LCP, no image. */}
          <section className="max-w-[46rem] py-20">
            <h1 className="text-[clamp(2.1rem,5.2vw,3.25rem)] leading-[1.05] font-semibold tracking-[-0.035em] text-balance text-strong">
              {SITE.tagline}
            </h1>
            <p className="mt-4 max-w-[38rem] text-[1.0625rem] text-muted-ink">{SITE.description}</p>
            <Install />
            <p className="mt-4 font-mono text-[0.6875rem] text-faint">
              <span className="mr-1.5 inline-block size-[5px] rounded-full bg-strong align-[1px]" />
              Self-hosted and free. Hosted beta rolling out.
            </p>
          </section>

          {SECTIONS.map((s) => (
            <section key={s.n} aria-labelledby={`s-${s.n}`} className="border-t border-line py-12">
              <p className={NUM}>{s.n}</p>
              <h2 id={`s-${s.n}`} className={H2}>
                {s.lead} <span className="font-normal text-faint">{s.rest}</span>
              </h2>
              <p className={SAY}>{s.say}</p>

              {s.artifact ? (
                <Artifact
                  title={s.artifact.title}
                  meta={s.artifact.meta}
                  code={s.artifact.code}
                  lang={s.artifact.lang}
                />
              ) : (
                <dl className="mt-6 grid gap-px overflow-hidden rounded-xl border border-line bg-line sm:grid-cols-3">
                  {PROMISES.map((p) => (
                    <div key={p.title} className="bg-card p-4">
                      <dt className="text-[0.8125rem] font-semibold tracking-tight text-strong">
                        {p.title}
                      </dt>
                      <dd className="mt-1.5 text-[0.8125rem] text-muted-ink">{p.body}</dd>
                    </div>
                  ))}
                </dl>
              )}
            </section>
          ))}

          <section aria-labelledby="runs-on" className="border-t border-line py-12">
            <p className={NUM}>06</p>
            <h2 id="runs-on" className={H2}>
              No migration.{' '}
              <span className="font-normal text-faint">
                sigiro plugs into what you already run.
              </span>
            </h2>
            <ul className="mt-6 flex flex-wrap gap-1.5">
              {RUNS_ON.map((item) => (
                <li
                  key={item}
                  className="rounded-md border border-line bg-card px-2.5 py-1.5 font-mono text-[0.6875rem] text-muted-ink"
                >
                  {item}
                </li>
              ))}
            </ul>
          </section>

          <section aria-labelledby="itself" className="border-t border-line py-12">
            <p className={NUM}>07</p>
            <h2 id="itself" className={H2}>
              It watches itself.{' '}
              <span className="font-normal text-faint">
                A pipeline blind to its own failures is not observability.
              </span>
            </h2>
            <p className={SAY}>
              sigiro emits telemetry for every ingest and every flush it performs, into itself. That
              is how a stalled writer, a leaked connection and a dropped channel were each caught —
              rather than surfacing days later as data that had quietly stopped arriving.
            </p>
          </section>

          <section aria-labelledby="faq" className="border-t border-line py-12">
            <p className={NUM}>FAQ</p>
            <h2 id="faq" className="mt-2.5 text-[1.375rem] font-semibold tracking-[-0.025em] text-strong">
              Questions.
            </h2>
            <Faq />
          </section>
        </main>

        <footer className="flex flex-wrap items-end justify-between gap-6 border-t border-line py-10">
          <div>
            <p className="font-semibold tracking-tight text-strong">{SITE.tagline}</p>
            <Link
              href="/docs/quickstart"
              prefetch={false}
              className="mt-3.5 inline-block rounded-lg bg-accent px-3.5 py-2 text-[0.8125rem] font-medium text-on-accent"
            >
              Get started
            </Link>
          </div>
          <nav aria-label="Footer" className="flex flex-wrap gap-4 font-mono text-[0.6875rem]">
            {NAV.map((item) => (
              <Link
                key={item.href}
                href={item.href}
                prefetch={false}
                className="text-faint hover:text-strong"
              >
                {item.label.toLowerCase()}
              </Link>
            ))}
            {SOCIAL.map((item) => (
              <a
                key={item.href}
                href={item.href}
                rel="me noopener"
                className="text-faint hover:text-strong"
              >
                {item.label.toLowerCase()}
              </a>
            ))}
            <a href="/llms.txt" className="text-faint hover:text-strong">
              llms.txt
            </a>
          </nav>
        </footer>
      </div>

      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(faqSchema) }}
      />
    </>
  );
}
