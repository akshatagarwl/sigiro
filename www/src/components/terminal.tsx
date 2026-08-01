'use client';

import {
  motion,
  useMotionTemplate,
  useMotionValueEvent,
  useScroll,
  useTransform,
} from 'motion/react';
import { useMemo, useRef, useState } from 'react';

/**
 * One terminal per step, stepped by scroll.
 *
 * An earlier version typed the transcript out character by character, bound to
 * scroll position. It was a good demo of scrubbing and a bad terminal: real
 * shells do not type their own output. You run a command and the output lands.
 * So nothing here animates text — a frame arrives with its command already run,
 * and sits at a fresh prompt, which is where a shell waits when it has finished.
 *
 * What scroll drives is which frame you are looking at, and the handoff between
 * frames. Both come off `scrollYProgress`, so both reverse on the way back.
 *
 * One step, one terminal, because when consecutive steps have different owners a
 * single shared shell says one actor ran all of them. A coding agent's work does
 * not happen in your sigiro shell.
 *
 * The rail is not decoration. Once the panel pins, the section heading has
 * scrolled away, so without it there is nothing on screen saying what the five
 * things are, which are ours, or how far through you are. §6: "a strip of cells
 * … with the accent marking where you are."
 *
 * Design constraints, from interior's DESIGN.md:
 * - §5h  React does not re-render at 60fps. The step index is React state; the
 *        handoff is MotionValues off `scrollYProgress` and never touches React.
 *        A previous version quantised progress to two decimals and drove the
 *        fade from state, which left the handoff about six frames long — §6a
 *        calls this exactly: "quantising the sweep would make the fill look
 *        like it was stuttering". Cells are the information, the sweep is the
 *        material, and only the cells are allowed to be discrete.
 * - §6a  `setStep(prev => prev === s ? prev : s)` on the discrete half.
 * - §5   the handoff uses the specified displacements — out at `y 6, blur(3px)`,
 *        in at `y 10, blur(6px)`.
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site.
 * - §3   no idle loops, so the resting cursor is a block that does not blink.
 * - §5h  a scroll-driven component needs no reduced-motion branch — every value
 *        resolves on the frame it is scrolled to. `fallback` exists for no-JS,
 *        not for reduced motion, and that is not an oversight.
 */

export type TerminalStep = {
  cmd: string;
  out: string;
  /** Names the step on the rail and in the caption — "Diagnose", "Connect". */
  label: string;
  /** A chip beside the label: whose shell this is. */
  badge?: string;
  /** Ink rather than muted ink, on the rail and the chip, for steps that are ours. */
  badgeStrong?: boolean;
  /** Prose under the well, swapped as the step changes. */
  note?: string;
};

/** 12.5px type on a 22px rhythm, explicit so the frame maths stays integer. */
const LINE = 22;
/** Vertical padding inside the well, top and bottom. */
const PAD = 24;

/**
 * Fraction of a step's scroll spent holding still before the handoff begins.
 * The remainder is the handoff: frame out over the first half, in over the
 * second.
 */
const HOLD = 0.72;

type Line = { text: string; cmd: boolean };

function plan(steps: readonly TerminalStep[]) {
  return steps.map((s) => {
    const lines: Line[] = [{ text: `$ ${s.cmd}`, cmd: true }];
    for (const text of s.out.split('\n')) lines.push({ text, cmd: false });
    // A shell that has finished sits at a fresh prompt. One more row for it.
    return { lines, height: (lines.length + 1) * LINE + PAD };
  });
}

/** Where a scroll position falls: which step, and how far into its handoff. */
function locate(v: number, count: number) {
  const scaled = Math.min(Math.max(v, 0), 0.9999) * count;
  const index = Math.floor(scaled);
  const frac = scaled - index;
  const handoff = index >= count - 1 || frac <= HOLD ? 0 : (frac - HOLD) / (1 - HOLD);
  return { index, handoff };
}

export function SteppedTerminal({
  steps,
  fallback,
  track,
}: {
  steps: readonly TerminalStep[];
  /** Shown where JavaScript never runs. */
  fallback: React.ReactNode;
  /**
   * Scroll distance the whole sequence occupies. Defaults to one viewport for
   * the pin plus 45vh of travel per step.
   *
   * It was 55vh while the transcript typed itself, because the reader was
   * waiting on the machine. Nothing waits now — a frame arrives complete — so
   * the only thing the travel has to cover is reading, which the reader paces
   * themselves by stopping.
   */
  track?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const frames = useMemo(() => plan(steps), [steps]);
  const { scrollYProgress } = useScroll({ target: ref, offset: ['start start', 'end end'] });

  // The discrete half: which frame's text is mounted. One render per step.
  const [shown, setShown] = useState(0);
  useMotionValueEvent(scrollYProgress, 'change', (v) => {
    const { index, handoff } = locate(v, steps.length);
    const next = handoff >= 0.5 ? index + 1 : index;
    setShown((prev) => (prev === next ? prev : next));
  });

  // The continuous half: never React state, so it runs at frame rate.
  const opacity = useTransform(scrollYProgress, (v) => {
    const { handoff: h } = locate(v, steps.length);
    if (!h) return 1;
    return h < 0.5 ? 1 - h / 0.5 : (h - 0.5) / 0.5;
  });
  const y = useTransform(scrollYProgress, (v) => {
    const { handoff: h } = locate(v, steps.length);
    if (!h) return 0;
    return h < 0.5 ? -6 * (h / 0.5) : 10 * (1 - (h - 0.5) / 0.5);
  });
  const blur = useTransform(scrollYProgress, (v) => {
    const { handoff: h } = locate(v, steps.length);
    if (!h) return 0;
    return h < 0.5 ? 3 * (h / 0.5) : 6 * (1 - (h - 0.5) / 0.5);
  });
  const filter = useMotionTemplate`blur(${blur}px)`;

  // Resizing rides the handoff too, where the content it would jump is faded.
  const height = useTransform(scrollYProgress, (v) => {
    const { index, handoff: h } = locate(v, steps.length);
    const a = frames[index].height;
    const b = frames[Math.min(index + 1, frames.length - 1)].height;
    return a + (b - a) * h;
  });

  const step = steps[shown];

  return (
    <>
      <noscript>{fallback}</noscript>

      <div
        ref={ref}
        className="relative"
        style={{ height: track ?? `calc(100vh + ${steps.length * 45}vh)` }}
      >
        <div className="sticky top-0 flex h-screen flex-col justify-center gap-3.5">
          {/* The rail: five named cells, ours in ink, the current one accented.
              This is the only thing on screen once the heading has scrolled off
              that says what the sequence is. */}
          <ol className="flex flex-wrap items-center gap-x-4 gap-y-1.5 px-1">
            {steps.map((s, i) => (
              <li key={s.label} className="flex items-center gap-1.5">
                <span
                  aria-hidden
                  className={`size-[5px] rounded-[1.5px] ${
                    i === shown ? 'bg-accent' : s.badgeStrong ? 'bg-ink' : 'bg-ink-3'
                  }`}
                />
                <span
                  className={`text-[11.5px] font-medium tracking-[-0.01em] ${
                    i === shown ? 'text-ink' : 'text-ink-3'
                  }`}
                >
                  {s.label}
                </span>
              </li>
            ))}
          </ol>

          <figure className="mat-panel w-full rounded-[14px] p-[5px]">
            <figcaption className="flex h-9 items-center gap-2.5 pr-[3px] pl-2.5">
              {step.badge ? (
                <span
                  className={`mat-cap inline-flex h-[22px] shrink-0 items-center rounded-[6px] px-2 text-[11.5px] font-medium ${
                    step.badgeStrong ? 'text-ink' : 'text-ink-3'
                  }`}
                >
                  {step.badge}
                </span>
              ) : null}
              <span className="text-[13px] font-medium tracking-[-0.01em] text-ink">
                {step.label}
              </span>
              <span className="meta ml-auto shrink-0 text-ink-3">
                {shown + 1} of {steps.length}
              </span>
            </figcaption>

            <motion.div
              className="mat-well overflow-hidden rounded-[9px] px-3.5 py-3"
              style={{ height }}
            >
              <motion.div
                aria-hidden
                className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
                style={{ opacity, y, filter }}
              >
                {frames[shown].lines.map((line, i) => (
                  <div
                    // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
                    key={i}
                    className={line.cmd ? 'text-ink' : 'text-ink-2'}
                    style={{ height: LINE }}
                  >
                    {line.text}
                  </div>
                ))}
                <div className="text-ink" style={{ height: LINE }}>
                  {'$ '}
                  <span className="ml-px inline-block h-[13px] w-[7px] bg-ink align-[-1px]" />
                </div>
              </motion.div>
            </motion.div>

            {step.note ? (
              <p className="mt-2 min-h-[3.4rem] max-w-[62ch] px-2.5 pb-1 text-[13px] leading-relaxed text-ink-2">
                {step.note}
              </p>
            ) : null}
          </figure>
        </div>
      </div>

      {/* Which frame is on screen depends on scroll position, and a screen reader
          has no scroll position. This is every session, whole, once. */}
      <pre className="sr-only">
        {frames.map((f) => f.lines.map((l) => l.text).join('\n')).join('\n\n')}
      </pre>
    </>
  );
}
