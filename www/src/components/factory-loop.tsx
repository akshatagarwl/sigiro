'use client';

import { motion, useInView, useReducedMotion } from 'motion/react';
import { useEffect, useRef, useState } from 'react';
import { LOOP } from '@/lib/site';

/**
 * Where sigiro sits in a software factory: five stages, each with its own
 * terminal, read straight down the page.
 *
 * The five commands are still one incident in order — a +48σ shift on checkout,
 * the suspect deploy, the agent's patch, the recovery, the playbook — but they
 * are five separate sessions rather than one transcript sliding behind a window.
 * Separate terminals put the prose back beside the command it explains, and cost
 * nothing to reason about: no sticky frame, no waypoints, no active-stage rule.
 * (That rule was wrong twice, in both directions, before this deleted it.)
 *
 * Two of the five stages visibly run something that is not sigiro, which is the
 * argument the section exists to make. It is watched, not asserted.
 *
 * On MagicUI's Terminal, which this could have used and does not: its chrome is
 * three `rounded-full` traffic lights, and §3 bans `rounded-full` and decorative
 * colour — red/yellow/green would be three more hues in a system with one accent.
 * It is also a bordered `max-w-lg` box, where interior's whole argument is that
 * materials replace borders faking compartments. What it has that is worth
 * having is `startOnView` + `sequence`: output arriving a line at a time. That is
 * the part adopted here, in about fifteen lines, with no component to vendor.
 *
 * From interior's DESIGN.md:
 * - §5    entrance is 0.22s at `opacity 0, scale 0.97, y 10, blur(6px)`.
 * - §5d   "physics beats taste" — a command printing its output is a real
 *         process, so the cadence between lines is linear, not eased.
 * - §3    no idle loops. Each terminal plays once, when you reach it.
 * - §22   the information arrives, the trip is skipped.
 *
 * Lines are chosen over characters deliberately. Typing a six-row table one
 * character at a time is a long wait to read something you could already read,
 * and it would put the page's slowest animation on its densest content.
 */

const EASE = [0.23, 1, 0.32, 1] as const;
const ENTER = { duration: 0.22, ease: EASE };

/** Linear, because output printing is a real process (§5d). */
const LINE_CADENCE = 0.055;

function Session({ cmd, out }: { cmd: string; out: string }) {
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const inView = useInView(ref, { once: true, amount: 0.3 });
  const [armed, setArmed] = useState(false);

  // Pre-mount and no-JS render the finished session; only a browser that ran
  // this effect ever hides a line, and it does so above the fold of this
  // section, so nothing is seen to blink.
  useEffect(() => {
    if (!reduced) setArmed(true);
  }, [reduced]);

  const lines = out.split('\n');
  const show = !armed || inView;

  return (
    <div ref={ref} className="font-mono text-[12.5px] leading-[1.7]">
      <motion.p
        initial={false}
        animate={show ? { opacity: 1, scale: 1, y: 0, filter: 'blur(0px)' } : { opacity: 0 }}
        transition={armed ? ENTER : { duration: 0 }}
        className="flex gap-2 whitespace-pre text-ink"
      >
        <span aria-hidden className="shrink-0 text-ink-3">
          $
        </span>
        <span>{cmd}</span>
      </motion.p>

      <div className="mt-1.5 whitespace-pre text-ink-2">
        {lines.map((line, i) => (
          <motion.div
            // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
            key={i}
            initial={false}
            animate={
              show
                ? { opacity: 1, scale: 1, y: 0, filter: 'blur(0px)' }
                : { opacity: 0, scale: 0.97, y: 10, filter: 'blur(6px)' }
            }
            transition={armed ? { ...ENTER, delay: 0.12 + i * LINE_CADENCE } : { duration: 0 }}
          >
            {/* A blank transcript line still has to occupy its row. */}
            {line === '' ? ' ' : line}
          </motion.div>
        ))}
      </div>
    </div>
  );
}

export function FactoryLoop() {
  return (
    <ol className="mt-8">
      {LOOP.map((s) => (
        <li key={s.title} className="scroll-mt-28 border-t border-hairline py-9 first:border-t-0">
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

          {/* The same shell every code block on the site is cut into: a panel at
              radius 14 holding a well at radius 9. 14 = 9 + 5. */}
          <figure className="mat-panel mt-5 rounded-[14px] p-[5px]">
            <figcaption className="meta flex h-8 items-center pl-2.5 text-ink-3">
              your shell
            </figcaption>
            <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3">
              <Session cmd={s.cmd} out={s.out} />
            </div>
          </figure>
        </li>
      ))}
    </ol>
  );
}
