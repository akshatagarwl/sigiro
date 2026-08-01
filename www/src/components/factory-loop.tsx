'use client';

import { AnimatePresence, motion, useReducedMotion } from 'motion/react';
import { useEffect, useRef, useState } from 'react';
import { LOOP } from '@/lib/site';

/**
 * Where sigiro sits in a software factory, told as one terminal session.
 *
 * The five stages are one continuous incident — detected, diagnosed, fixed,
 * verified, written down — and the terminal at the top of the section shows
 * whichever command belongs to the stage you are reading. Two of the five run
 * something that is not sigiro, which is the argument the section exists to make
 * and is now a thing you watch rather than a claim you read.
 *
 * This replaced a `ScrollSpy` rail. The rail reported which stage you were on;
 * the terminal reports that *and* carries the payload, so the rail was a label
 * for something that can just show itself.
 *
 * From interior's DESIGN.md:
 * - §5a  two curves. Arrivals use EASE, departures LEAVE, and a departure is
 *        always shorter — 0.18s out against 0.22s in.
 * - §3   no `rounded-full`. So there is no traffic-light chrome and no fake
 *        window: a terminal here is a `mat-well` with a prompt glyph, nothing
 *        more. It is the same well every code block on the site is cut into.
 * - §3   no idle loops. Nothing types on a timer; the session advances only
 *        because you scrolled.
 * - §22  the information arrives, the trip is skipped.
 *
 * No-JS and pre-mount render the whole session stacked, one terminal under each
 * stage, so the transcript is in the HTML for anyone whose JavaScript never runs.
 * `armed` flips after mount and swaps that for the sticky single-screen view.
 */

const EASE = [0.23, 1, 0.32, 1] as const;
const LEAVE = [0.4, 0, 1, 1] as const;

const IN = { duration: 0.22, ease: EASE };
const OUT = { duration: 0.18, ease: LEAVE };

/**
 * Tallest `out` is six lines plus a blank, and the sticky frame must not resize
 * as the session advances — a terminal that grows and shrinks under a fixed
 * viewport reads as the page twitching, not as a program running.
 */
const FRAME = 'min-h-[188px]';

function Terminal({ cmd, out }: { cmd: string; out: string }) {
  return (
    <div className="font-mono text-[12.5px] leading-[1.7]">
      <p className="flex gap-2 text-ink">
        <span aria-hidden className="shrink-0 text-ink-3">
          $
        </span>
        <span className="min-w-0">{cmd}</span>
      </p>
      <pre className="mt-1.5 whitespace-pre text-ink-2">{out}</pre>
    </div>
  );
}

export function FactoryLoop() {
  const reduced = useReducedMotion();
  const [armed, setArmed] = useState(false);
  const [active, setActive] = useState(0);
  const stages = useRef<(HTMLLIElement | null)[]>([]);
  const sticky = useRef<HTMLDivElement>(null);

  useEffect(() => setArmed(true), []);

  /**
   * The active stage is whichever one sits closest to the middle of the space
   * *below* the terminal — that space is the whole of what a reader can see, so
   * its centre is where they are actually reading.
   *
   * The obvious rule, "the last stage whose top has passed a fixed line", is
   * wrong here and was wrong in the first draft: with the line at 42% of the
   * viewport, centring a stage put its top at 307px against a line at 302px, so
   * every stage resolved to the one before it. Any fixed line has some version of
   * that bug, because the sticky terminal's height decides how much room is left.
   * Measuring the terminal each frame removes the constant to get wrong.
   *
   * A scroll listener rather than five observers: one rAF-throttled read of five
   * `getBoundingClientRect`s is cheaper than five observer callbacks that each
   * have to be reconciled into a single answer.
   */
  useEffect(() => {
    if (!armed) return;
    let frame = 0;
    const read = () => {
      frame = 0;
      const below = sticky.current?.getBoundingClientRect().bottom ?? 0;
      const focus = below + (window.innerHeight - below) / 2;
      let next = 0;
      let best = Number.POSITIVE_INFINITY;
      stages.current.forEach((el, i) => {
        if (!el) return;
        const r = el.getBoundingClientRect();
        const gap = Math.abs(r.top + r.height / 2 - focus);
        if (gap < best) {
          best = gap;
          next = i;
        }
      });
      setActive(next);
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(read);
    };
    read();
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      window.removeEventListener('scroll', onScroll);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [armed]);

  const stage = LOOP[active];

  return (
    <div className="mt-8">
      {armed ? (
        <div ref={sticky} className="sticky top-0 z-10 -mx-1 bg-panel px-1 py-3 relative">
          <div className="mat-panel rounded-[14px] p-[5px]">
            <p className="meta flex h-8 items-center gap-2.5 pl-2.5 text-ink-3">
              <span className="truncate">
                your shell · {active + 1} of {LOOP.length}
              </span>
            </p>
            <div
              className={`mat-well scroll-inset relative overflow-hidden rounded-[9px] px-3.5 py-3 ${FRAME}`}
            >
              <AnimatePresence initial={false} mode="popLayout">
                <motion.div
                  key={stage.title}
                  initial={reduced ? false : { opacity: 0, y: 10, filter: 'blur(6px)' }}
                  animate={{ opacity: 1, y: 0, filter: 'blur(0px)' }}
                  // A departure is always shorter than an arrival (§5a), so the
                  // exit carries its own transition rather than inheriting IN.
                  exit={
                    reduced
                      ? { opacity: 0, transition: { duration: 0 } }
                      : { opacity: 0, y: -6, filter: 'blur(3px)', transition: OUT }
                  }
                  transition={reduced ? { duration: 0 } : IN}
                >
                  <Terminal cmd={stage.cmd} out={stage.out} />
                </motion.div>
              </AnimatePresence>
            </div>
          </div>
          {/* Prose passing under the terminal was being bisected at its bottom
              edge — a half-line of type reads as a rendering fault, not as
              content scrolling away. §5g names the gradient overlay as the fix
              for exactly this (and warns off animating `mask-image`); nothing
              here moves, so it stays a static strip. */}
          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-full h-7 bg-gradient-to-b from-panel to-transparent"
          />
        </div>
      ) : null}

      <ol>
        {LOOP.map((s, i) => (
          <li
            key={s.title}
            ref={(el) => {
              stages.current[i] = el;
            }}
            /* Enough height that the sticky terminal has travel to report. Below
               about 13rem two stages share the reading line and the session
               flickers between them on every scroll. */
            className="flex min-h-[13rem] scroll-mt-28 flex-col justify-center border-t border-hairline py-9 first:border-t-0"
          >
            <p className="flex items-center gap-2.5">
              {/* Ink and depth, not accent: "this stage is ours" is a persistent
                  state, and if every marked thing were blue, focus would stop
                  meaning anything. */}
              <span
                className={`mat-cap inline-flex h-[22px] items-center rounded-[6px] px-2 text-[11.5px] font-medium ${
                  s.ours ? 'text-ink' : 'text-ink-3'
                }`}
              >
                {s.owner}
              </span>
            </p>
            <h3 className="mt-3.5 text-[20px] font-medium tracking-[-0.03em] text-ink">{s.title}</h3>
            <p className="mt-2.5 max-w-[62ch] text-[13.5px] leading-relaxed text-ink-2">{s.say}</p>

            {/* No-JS: the whole session, stacked. */}
            {armed ? null : (
              <div className="mat-well scroll-inset mt-4 overflow-x-auto rounded-[9px] px-3.5 py-3">
                <Terminal cmd={s.cmd} out={s.out} />
              </div>
            )}
          </li>
        ))}
      </ol>
    </div>
  );
}
