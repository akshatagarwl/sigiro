'use client';

import {
  motion,
  useMotionTemplate,
  useMotionValueEvent,
  useScroll,
  useTransform,
} from 'motion/react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { SkeletonSwap } from '@/components/interior/skeleton-swap';
import { TaskSteps } from '@/components/interior/task-steps';
import type { SessionLine } from '@/lib/highlight';

/**
 * One terminal per step, stepped by scroll, with a task list beside it.
 *
 * An earlier version typed the transcript out character by character, bound to
 * scroll position. It was a good demo of scrubbing and a bad terminal: real
 * shells do not type their own output. You run a command and the output lands.
 * So nothing types — but a command does take a beat to run, which is what
 * `SkeletonSwap` is for: arrive at a frame, the shell is busy, then the output
 * appears. Its 120ms delay and 380ms floor are the reason this reads as a
 * command running rather than as a flicker (§19).
 *
 * `TaskSteps` carries the argument the section exists to make. Five stages, the
 * ones you have passed ticked off, the one you are on running, the rest waiting
 * — and `meta` naming the owner of each, so "two of these five are ours" is
 * something you watch accumulate rather than a claim in a caption. It also
 * brings `aria-current="step"` and a debounced live region, which the rail I
 * hand-rolled did not have.
 *
 * Both are vendored verbatim into components/interior/, so they write literal
 * stone and emerald values rather than our tokens. That is §1b, on purpose: a
 * copied component must not depend on the host's palette.
 *
 * Design constraints, from DESIGN.md (now vendored at www/DESIGN.md):
 * - §5h  React does not re-render at 60fps. The step index is React state; the
 *        handoff is MotionValues off `scrollYProgress` and never touches React.
 *        A previous version quantised progress to two decimals and drove the
 *        fade from state, leaving the handoff ~6 frames long — §6a: "quantising
 *        the sweep would make the fill look like it was stuttering".
 * - §6a  `setStep(prev => prev === s ? prev : s)` on the discrete half.
 * - §5   the handoff uses the specified displacements — out at `y 6, blur(3px)`,
 *        in at `y 10, blur(6px)`.
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site —
 *        and, since `highlightSession`, wearing the same Shiki colours. The
 *        markup arrives already highlighted: this component is a client
 *        component and Shiki is a server-side dependency, so what crosses is
 *        one string of HTML per line, never a highlighter.
 * - §5h  a scroll-driven component needs no reduced-motion branch — every value
 *        resolves on the frame it is scrolled to. `fallback` covers no-JS only,
 *        and that is deliberate rather than an oversight.
 */

export type TerminalStep = {
  /**
   * The session, one line per line, highlighted on the server by
   * `highlightSession` — the `$ command` row first, then its output.
   */
  lines: readonly SessionLine[];
  /** Names the step in the task list and the caption — "Diagnose", "Connect". */
  label: string;
  /** Whose shell this is. Shown against the step once it is done. */
  badge?: string;
  /** Ink rather than muted ink on the chip, for steps that are ours. */
  badgeStrong?: boolean;
  /**
   * Short form of `badge`, for the task list's `meta` column.
   *
   * That column is `shrink-0` against a `flex-1 truncate` label, so a long owner
   * squeezes the label to zero width and the step loses its name — invisibly,
   * because meta is `opacity-0` until the step is done. "your pipeline, on our
   * evidence" did exactly that to "Verify". Falls back to `badge`.
   */
  badgeShort?: string;
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

/**
 * How long a command "runs" on arrival. Short, because it is a beat rather than
 * a claim about latency — `SkeletonSwap` stretches it to its own 380ms floor so
 * the skeleton never strobes.
 */
const RUN_MS = 300;

function plan(steps: readonly TerminalStep[]) {
  return steps.map(({ lines }) => ({
    lines,
    // A shell that has finished sits at a fresh prompt. One more row for it.
    height: (lines.length + 1) * LINE + PAD,
  }));
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
   * waiting on the machine. Nothing waits now beyond the run beat, so the only
   * thing the travel has to cover is reading, which the reader paces by
   * stopping.
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

  // Arriving at a frame runs its command. One timer, reset whenever the frame
  // changes, so scrubbing quickly does not stack them up.
  const [ran, setRan] = useState(false);
  useEffect(() => {
    setRan(false);
    const t = setTimeout(() => setRan(true), RUN_MS);
    return () => clearTimeout(t);
  }, [shown]);

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
  const frame = frames[shown];

  const tasks = useMemo(
    () => steps.map((s) => ({ id: s.label, label: s.label, meta: s.badgeShort ?? s.badge })),
    [steps],
  );

  return (
    <>
      <noscript>{fallback}</noscript>

      <div
        ref={ref}
        className="relative"
        style={{ height: track ?? `calc(100vh + ${steps.length * 45}vh)` }}
      >
        <div className="sticky top-0 flex h-screen flex-col justify-center gap-5 lg:grid lg:grid-cols-[190px_1fr] lg:items-center">
          {/* The task list is the only thing on screen carrying the shape of the
              argument once the section heading has scrolled away. */}
          <TaskSteps steps={tasks} current={shown} label="The five stages" />

          <figure className="mat-panel w-full min-w-0 rounded-[14px] p-[5px]">
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
              <motion.div style={{ opacity, y, filter }}>
                <SkeletonSwap
                  ready={ran}
                  lines={frame.lines.length + 1}
                  lineHeight={LINE}
                  reserve={(frame.lines.length + 1) * LINE}
                  className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
                >
                  {/* One box per line, so the frame height stays integer. The
                      `shiki` class is on the line rather than on the well: its
                      `span` rule forces a transparent background, which would
                      erase the block cursor below. */}
                  {frame.lines.map((line, i) => (
                    <div
                      // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
                      key={i}
                      className="shiki"
                      style={{ height: LINE }}
                      // Shiki output, generated from literals in this repo.
                      dangerouslySetInnerHTML={{ __html: line.html }}
                    />
                  ))}
                  <div className="text-ink-3" style={{ height: LINE }}>
                    {'$ '}
                    <span className="ml-px inline-block h-[13px] w-[7px] bg-ink align-[-1px]" />
                  </div>
                </SkeletonSwap>
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
