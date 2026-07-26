import type { Metadata } from 'next';
import Link from 'next/link';
import { Artifact } from '@/components/artifact';
import { Faq } from '@/components/faq';
import { Integrations } from '@/components/integrations';
import { Button } from '@/components/ui/button';
import { SiteFooter, SiteHeader } from '@/components/site-chrome';
import { FAQ, PROMISES, SECTIONS, SITE } from '@/lib/site';
import { jsonLd } from '@/lib/json-ld';

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

export default function Home() {
  return (
    <>
      <div className="mx-auto max-w-[60rem] px-6">
        <SiteHeader />

        <main id="main">
          {/* Hero: text LCP, no image. */}
          <section className="max-w-[46rem] py-20">
            <h1 className="text-[clamp(2.1rem,5.2vw,3.25rem)] leading-[1.05] font-semibold tracking-[-0.035em] text-balance text-strong">
              {SITE.tagline}
            </h1>
            <p className="mt-4 max-w-[38rem] text-[1.0625rem] text-muted-ink">{SITE.description}</p>
            {/* Buttons, not a snippet. `@kibo-ui/snippet` renders
                `<pre className="truncate">`, so a long command is cut off — and the
                install command belongs in 01 Connect, where the code block already
                scrolls and carries its own copy button. */}
            <div className="mt-8 flex flex-wrap items-center gap-2.5">
              <Button size="lg" render={<Link href="/docs/quickstart" prefetch={false} />}>
                Get started
              </Button>
              <Button size="lg" variant="outline" render={<Link href="/docs" prefetch={false} />}>
                Read the docs
              </Button>
            </div>
            <p className="mt-5 font-mono text-[0.6875rem] text-faint">
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
            <Integrations />
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

        <SiteFooter />
      </div>

      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: jsonLd(faqSchema) }}
      />
    </>
  );
}
