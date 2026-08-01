import type { Metadata } from 'next';
import Link from 'next/link';
import { Artifact } from '@/components/artifact';
import { Faq } from '@/components/faq';
import { FactoryLoop } from '@/components/factory-loop';
import { Integrations } from '@/components/integrations';
import { ProofDemo } from '@/components/proof-demo';
import { PRIMARY, SECONDARY } from '@/components/site/action';
import { PageShell, SiteFooter, SiteHeader } from '@/components/site-chrome';
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

/* The type ramp, from the design language and not invented here: 27px medium at
   -0.03em for a page or section title, 15px for a lede, 13.5px for body.
   Headings are medium, never bold — the negative tracking does the work that
   weight would do badly. */
const H2 = 'mt-3 max-w-[34ch] text-[27px] leading-[1.2] font-medium tracking-[-0.03em] text-ink';
const LEDE = 'mt-3 max-w-[62ch] text-[15px] leading-relaxed text-ink-2';
const SECTION = 'border-t border-hairline px-4 py-14 sm:px-8 lg:px-12';

export default function Home() {
  return (
    <>
      <PageShell>
        <SiteHeader />

        {/* The measure. The panel is as wide as the window, and a 1400px column
            of 13.5px prose is not a reading measure — so the document inside it
            is centred and capped, and the section hairlines span the document
            rather than the panel. */}
        <main id="main" className="mx-auto w-full min-w-0 max-w-[996px]">
          {/* Hero: text LCP, no image. */}
          <section className="px-4 py-16 sm:px-8 sm:py-24 lg:px-12">
            <h1 className="max-w-[24ch] text-[clamp(30px,7.2vw,32px)] leading-[1.08] font-medium tracking-[-0.04em] text-balance text-ink sm:text-[clamp(32px,5.6vw,52px)]">
              {SITE.tagline}
            </h1>
            <p className="mt-6 max-w-[52ch] text-[13.5px] leading-[1.7] text-ink-2 sm:mt-8 sm:text-[15px]">
              {SITE.description}
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3 sm:mt-10">
              <Link href="/docs/quickstart" prefetch={false} className={PRIMARY}>
                Get started
              </Link>
              <Link href="/docs" prefetch={false} className={SECONDARY}>
                Read the docs
              </Link>
            </div>
            {/* A cell, not a dot: there are no pills in this palette, and 5px at
                a 1.5px radius is the language's own unit. */}
            <p className="meta mt-6 flex items-center gap-2 text-ink-3">
              <span aria-hidden className="size-[5px] rounded-[1.5px] bg-accent" />
              Free, self-hosted, and it never phones home.
            </p>
          </section>

          {/* Where this fits, before how to use it. A reader whose agent already
              writes code knows the fix stage is solved; the question worth
              answering first is which stage is still missing. */}
          <section aria-labelledby="loop" className={SECTION}>
            <p className="label">the loop</p>
            <h2 id="loop" className={H2}>
              Five stages.{' '}
              <span className="text-ink-3">
                You do not have a factory until every one is covered.
              </span>
            </h2>
            <p className={LEDE}>
              Detect, diagnose, fix, verify, learn. The fix stage got solved first and loudest,
              which is why the loop breaks upstream of it: your agent can write the patch, but
              nothing tells it what broke or whether the patch worked. sigiro is two of these five
              stages, and the evidence the third one needs.
            </p>
            <FactoryLoop />
          </section>

          {SECTIONS.map((s) => (
            <section key={s.n} aria-labelledby={`s-${s.n}`} className={SECTION}>
              <p className="meta text-ink-3">{s.n}</p>
              <h2 id={`s-${s.n}`} className={H2}>
                {s.lead} <span className="text-ink-3">{s.rest}</span>
              </h2>
              <p className={LEDE}>{s.say}</p>

              {/* 04 is the one section whose claim is a demonstration rather
                  than a picture: "every answer carries the query behind it" is
                  proved by running it, not by printing it. Same finding, staged. */}
              {s.n === '04' ? (
                <ProofDemo finding={s.artifact.code} lang={s.artifact.lang} />
              ) : (
                <Artifact
                  title={s.artifact.title}
                  meta={s.artifact.meta}
                  code={s.artifact.code}
                  lang={s.artifact.lang}
                />
              )}
            </section>
          ))}

          <section aria-labelledby="runs-on" className={SECTION}>
            <p className="meta text-ink-3">06</p>
            <h2 id="runs-on" className={H2}>
              No migration.{' '}
              <span className="text-ink-3">sigiro plugs into what you already run.</span>
            </h2>
            <Integrations />
          </section>

          {/* The five things users say they care about, plus dogfooding as the
              evidence we mean them. This absorbed the old standalone "it watches
              itself" section: it was making one claim at the length of six. */}
          <section aria-labelledby="promises" className={SECTION}>
            <p className="meta text-ink-3">07</p>
            <h2 id="promises" className={H2}>
              The short version.{' '}
              <span className="text-ink-3">
                Free to run, nothing to maintain, and it stays on your infrastructure.
              </span>
            </h2>
            {/* Six wells with a real gap between them. `gap-px` on a tinted parent
                is a border faking a compartment; this is the material saying it. */}
            <dl className="mt-6 grid gap-2.5 sm:grid-cols-3">
              {PROMISES.map((p) => (
                <div key={p.title} className="mat-well rounded-[11px] p-3.5">
                  <dt className="text-[13px] font-medium tracking-[-0.01em] text-ink">{p.title}</dt>
                  <dd className="mt-1.5 text-[12.5px] leading-relaxed text-ink-3">{p.body}</dd>
                </div>
              ))}
            </dl>
          </section>

          <section aria-labelledby="faq" className={SECTION}>
            <p className="label">faq</p>
            <h2 id="faq" className={H2}>
              Questions.
            </h2>
            <Faq />
          </section>
        </main>

        <SiteFooter />
      </PageShell>

      <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: jsonLd(faqSchema) }} />
    </>
  );
}
