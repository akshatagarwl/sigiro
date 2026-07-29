'use client';

import { useEffect, useRef, useState } from 'react';
import { LOOP } from '@/lib/site';

/**
 * Where sigiro sits in a software factory, told by scrolling.
 *
 * The diagram is a vertical chain because the page scrolls vertically: as a step's
 * prose comes into view its node lights up and the line reaching it fills, so the
 * reader's own scroll is the animation. Nothing loops on a timer — ambient motion
 * on a diagram is decoration, and this one is an argument.
 *
 * Progressive enhancement, deliberately: every step and every node renders on the
 * server, and with no JavaScript the reader gets the whole list with the first
 * stage marked. The observer only moves the marker.
 *
 * The diagram is `aria-hidden`: it restates the list beside it, and a screen
 * reader should not have to hear the same five stages twice. The prose is the
 * content.
 */
export function FactoryLoop() {
  const [active, setActive] = useState(0);
  const steps = useRef<(HTMLLIElement | null)[]>([]);

  useEffect(() => {
    // One observer, no scroll handler: the whole point is to keep the main thread
    // free. A step becomes active when it crosses the middle of the viewport.
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const index = steps.current.indexOf(entry.target as HTMLLIElement);
          if (index !== -1) setActive(index);
        }
      },
      // A zero-height band on the viewport's exact middle: with a 10% band two
      // adjacent steps can both intersect it and the later one wins, which put the
      // marker a stage ahead of the prose being read.
      { rootMargin: '-50% 0px -50% 0px' },
    );

    for (const step of steps.current) if (step) observer.observe(step);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="mt-10 grid gap-10 lg:grid-cols-[19rem_1fr] lg:gap-16">
      {/* Sticky on both, but not the same object on both: see Chain. */}
      <div className="sticky top-0 z-10 self-start bg-bg/90 py-3 backdrop-blur lg:top-24 lg:bg-transparent lg:py-0 lg:backdrop-blur-none">
        <Chain active={active} />
      </div>

      <ol className="flex flex-col">
        {LOOP.map((stage, i) => (
          <li
            key={stage.n}
            ref={(el) => {
              steps.current[i] = el;
            }}
            className="flex min-h-[14rem] flex-col justify-center border-t border-line py-10 first:border-t-0 lg:min-h-[19rem]"
          >
            <p className="font-mono text-[0.6875rem] tracking-[0.12em] text-faint uppercase">
              {stage.n} · {stage.owner}
            </p>
            <h3
              className={`mt-3 text-[1.375rem] font-semibold tracking-[-0.025em] transition-colors duration-300 ${
                i === active ? 'text-strong' : 'text-muted-ink'
              }`}
            >
              {stage.title}
            </h3>
            <p className="mt-3 max-w-[34rem] text-[0.9375rem] text-body">{stage.say}</p>
            <p className="mt-4 font-mono text-[0.8125rem] text-faint">
              {stage.ours ? <span className="text-flow">→ </span> : null}
              {stage.detail}
            </p>
          </li>
        ))}
      </ol>
    </div>
  );
}

/**
 * The five stages, drawn twice, because a phone and a desktop need different
 * objects.
 *
 * Wide: the full chain, each stage named with its owner under it. Ours carry a
 * filled rail and the accent; the two that belong to the reader's own tools carry an
 * outline — the claim is that this is two stages of five, not the whole loop.
 *
 * Narrow: five bars and a counter. Five *labelled* pills in a row cannot shrink
 * below about 480px, so on a 412px screen they pushed the whole document wider and
 * every paragraph on the page clipped at the right edge. The names are in the
 * heading directly below anyway; here the rail only has to say how far along you
 * are, and which stages are ours.
 */
function Chain({ active }: { active: number }) {
  return (
    <>
      <div aria-hidden className="flex items-center gap-3 lg:hidden">
        <div className="flex flex-1 gap-1.5">
          {LOOP.map((stage, i) => (
            <span
              key={stage.n}
              className={`h-1 flex-1 rounded-full transition-colors duration-300 ${
                i === active
                  ? 'bg-strong'
                  : stage.ours
                    ? 'bg-flow/40'
                    : i < active
                      ? 'bg-line-strong'
                      : 'bg-line'
              }`}
            />
          ))}
        </div>
        <span className="shrink-0 font-mono text-[0.625rem] tracking-[0.08em] text-faint uppercase">
          {LOOP[active]?.n} / {LOOP.length.toString().padStart(2, '0')}
        </span>
      </div>

      <div aria-hidden className="dot-grid hidden rounded-xl border border-line p-5 lg:block">
        <ol className="flex flex-col">
          {LOOP.map((stage, i) => {
            const isActive = i === active;
            const reached = i <= active;
            return (
              <li key={stage.n}>
                {/* The connector into this node, filled once the reader reaches it. */}
                {i > 0 ? (
                  <div className="h-5 pl-[1.4rem]">
                    <div
                      className={`h-full w-px transition-colors duration-500 ${
                        reached ? 'bg-flow' : 'bg-line-strong'
                      }`}
                    />
                  </div>
                ) : null}

                <div
                  className={`flex items-center gap-2.5 rounded-lg border px-3 py-2 transition-colors duration-300 ${
                    isActive
                      ? 'border-flow bg-flow-tint'
                      : reached
                        ? 'border-line bg-card'
                        : 'border-line bg-card/40'
                  }`}
                >
                  <span
                    className={`h-6 w-[3px] shrink-0 rounded-full transition-colors duration-300 ${
                      stage.ours ? 'bg-flow' : reached ? 'bg-line-strong' : 'bg-line'
                    }`}
                  />
                  <span className="min-w-0">
                    <span
                      className={`block truncate text-[0.8125rem] font-medium transition-colors duration-300 ${
                        isActive ? 'text-flow' : 'text-strong'
                      }`}
                    >
                      {stage.title}
                    </span>
                    <span className="block font-mono text-[0.625rem] tracking-[0.08em] text-faint uppercase">
                      {stage.owner}
                    </span>
                  </span>
                </div>
              </li>
            );
          })}
        </ol>

        {/* The loop closing. Dashed, because it is the one edge that is not a handoff
            between tools — it is the same service coming back around with a new
            baseline. */}
        <div className="mt-4 flex items-center gap-2 border-t border-dashed border-flow/40 pt-3">
          <span className="font-mono text-[0.625rem] tracking-[0.08em] text-flow uppercase">
            back to 01
          </span>
          <span className="font-mono text-[0.625rem] text-faint">new baseline</span>
        </div>
      </div>
    </>
  );
}
