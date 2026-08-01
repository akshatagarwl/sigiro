'use client';

import { useMotionValueEvent, useReducedMotion, useScroll, useTransform } from 'motion/react';
import { useMemo, useRef, useState } from 'react';
import { LOOP } from '@/lib/site';

/**
 * The factory loop as one pinned terminal that types itself as you scroll.
 *
 * Scroll position drives the cursor directly: `useScroll` over the tall track
 * gives 0→1, that maps to a character index in the whole session, and the
 * transcript is sliced at it. Scroll up and it un-types. Nothing here runs on a
 * timer, so the animation has no duration of its own — it is entirely a function
 * of where the page is, which is what makes it scrub rather than play.
 *
 * The five commands are one incident in order: a +48σ shift on checkout, the
 * suspect deploy, the agent's patch, the recovery, the playbook. Two of them
 * visibly run something that is not sigiro, which is the argument the section
 * exists to make.
 *
 * On MagicUI's Terminal, which was installed, read and then removed: its engine
 * is `setInterval` typing plus `useInView({once: true})`, with a sequence context
 * chained off `onAnimationComplete`. There is no scroll input anywhere in the
 * file. It plays once when it appears and cannot reverse, so scrubbing it would
 * mean replacing every part of it that does anything, leaving the three
 * `rounded-full` traffic lights §3 bans and a `<pre>`. The disagreement is
 * mechanical, not stylistic. (It also imports `@/lib/utils`, which this repo does
 * not have, so it would not have typechecked as shipped.)
 *
 * From interior's DESIGN.md:
 * - §22   scroll-driven transforms are the one accepted exception to the
 *         `useReducedMotion` rule (`sticky-header` does the same). Reduced motion
 *         still gets a full, readable transcript — it just gets it stacked and
 *         static, with no pinning and nothing to scrub.
 * - §3    no `rounded-full`, so no traffic-light chrome; this is the same
 *         `mat-well` every code block on the site is cut into.
 * - §5g   a gradient overlay, never an animated mask, for the fade at the edges.
 *
 * No-JS gets the same stacked fallback as reduced motion, so the whole argument
 * and the whole session are in the HTML either way.
 */

/** Explicit so the auto-scroll maths is integer: 12.5px type on a 22px rhythm. */
const LINE = 22;
const WINDOW_LINES = 16;
const WINDOW = LINE * WINDOW_LINES;

/** Keep the cursor this far off the bottom, so you always see what comes next. */
const LOOKAHEAD = 4;

type Row = { text: string; cmd: boolean; stage: number; start: number };

/** The whole session, flattened to rows with cumulative character offsets. */
function useTranscript() {
  return useMemo(() => {
    const rows: Row[] = [];
    let chars = 0;
    LOOP.forEach((s, stage) => {
      if (stage > 0) {
        rows.push({ text: '', cmd: false, stage, start: chars });
        chars += 1;
      }
      const cmd = `$ ${s.cmd}`;
      rows.push({ text: cmd, cmd: true, stage, start: chars });
      chars += cmd.length + 1;
      for (const line of s.out.split('\n')) {
        rows.push({ text: line, cmd: false, stage, start: chars });
        chars += line.length + 1;
      }
    });
    return { rows, total: chars };
  }, []);
}

function Stacked() {
  return (
    <ol className="mt-8">
      {LOOP.map((s) => (
        <li key={s.title} className="border-t border-hairline py-9 first:border-t-0">
          <p className="flex items-center gap-2.5">
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
          <figure className="mat-panel mt-5 rounded-[14px] p-[5px]">
            <figcaption className="meta flex h-8 items-center pl-2.5 text-ink-3">
              your shell
            </figcaption>
            <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[22px]">
              <p className="flex gap-2 whitespace-pre text-ink">
                <span aria-hidden className="shrink-0 text-ink-3">
                  $
                </span>
                <span>{s.cmd}</span>
              </p>
              <pre className="whitespace-pre text-ink-2">{s.out}</pre>
            </div>
          </figure>
        </li>
      ))}
    </ol>
  );
}

export function FactoryLoop() {
  const reduced = useReducedMotion();
  const track = useRef<HTMLDivElement>(null);
  const { rows, total } = useTranscript();

  const { scrollYProgress } = useScroll({
    target: track,
    offset: ['start start', 'end end'],
  });

  /**
   * The last stretch of the track is dead on purpose: the session finishes with
   * a little scroll left, so the final command is readable before the pin
   * releases instead of flicking away on the frame it completes.
   */
  const cursor = useTransform(scrollYProgress, [0, 0.88], [0, total], { clamp: true });

  const [typed, setTyped] = useState(0);
  useMotionValueEvent(cursor, 'change', (v) => {
    const next = Math.round(v);
    setTyped((prev) => (prev === next ? prev : next));
  });

  // Which row the cursor is inside, and therefore which stage is running.
  let at = 0;
  for (let i = 0; i < rows.length; i++) {
    if (typed >= rows[i].start) at = i;
  }
  const stage = LOOP[rows[at]?.stage ?? 0];
  const offset = Math.max(0, at - (WINDOW_LINES - LOOKAHEAD)) * LINE;

  if (reduced) return <Stacked />;

  return (
    <>
      {/* The stacked session, for no-JS and for anything that never runs the
          scroll handler. Hidden the moment CSS is available, which is also the
          only condition under which the pinned version can work at all. */}
      <noscript>
        <Stacked />
      </noscript>

      <div ref={track} className="relative mt-8 h-[340vh]">
        <div className="sticky top-0 flex h-screen items-center">
          <figure className="mat-panel w-full rounded-[14px] p-[5px]">
            <figcaption className="flex h-9 items-center gap-2.5 pr-[3px] pl-2.5">
              <span
                className={`mat-cap inline-flex h-[22px] shrink-0 items-center rounded-[6px] px-2 text-[11.5px] font-medium ${
                  stage.ours ? 'text-ink' : 'text-ink-3'
                }`}
              >
                {stage.owner}
              </span>
              <span className="text-[13px] font-medium tracking-[-0.01em] text-ink">
                {stage.title}
              </span>
              <span className="meta ml-auto shrink-0 text-ink-3">
                {LOOP.indexOf(stage) + 1} of {LOOP.length}
              </span>
            </figcaption>

            <div
              className="mat-well relative overflow-hidden rounded-[9px] px-3.5 py-3"
              style={{ height: WINDOW }}
            >
              <div
                className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
                style={{ transform: `translateY(${-offset}px)` }}
              >
                {rows.map((row, i) => {
                  const shown =
                    typed >= row.start + row.text.length
                      ? row.text
                      : typed > row.start
                        ? row.text.slice(0, typed - row.start)
                        : '';
                  return (
                    <div
                      // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
                      key={i}
                      className={row.cmd ? 'text-ink' : 'text-ink-2'}
                      style={{ height: LINE }}
                    >
                      {shown}
                      {/* The cursor sits on the row being typed. A block, not a
                          blinking caret: §3 bans idle loops, and a caret that
                          blinks on a timer is one. */}
                      {typed > row.start && typed < row.start + row.text.length ? (
                        <span aria-hidden className="ml-px inline-block h-[13px] w-[7px] bg-ink" />
                      ) : null}
                    </div>
                  );
                })}
              </div>

              {/* §5g: a gradient, not a mask, so a line leaving the top or
                  arriving at the bottom fades instead of being cut through. */}
              <div
                aria-hidden
                className="pointer-events-none absolute inset-x-0 top-0 h-6 bg-gradient-to-b from-well to-transparent"
              />
              <div
                aria-hidden
                className="pointer-events-none absolute inset-x-0 bottom-0 h-8 bg-gradient-to-t from-well to-transparent"
              />
            </div>

            {/* The prose for the stage being typed. Kept out of the transcript so
                the transcript stays something sigiro would really print. */}
            <p className="mt-2 min-h-[3.4rem] max-w-[62ch] px-2.5 pb-1 text-[13px] leading-relaxed text-ink-2">
              {stage.say}
            </p>
          </figure>
        </div>
      </div>
    </>
  );
}
