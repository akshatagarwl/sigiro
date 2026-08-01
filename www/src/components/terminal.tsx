'use client';

import { useMotionValueEvent, useReducedMotion, useScroll } from 'motion/react';
import { useMemo, useRef, useState } from 'react';

/**
 * One terminal per step, stepped by scroll.
 *
 * An earlier version typed the transcript out character by character, bound to
 * scroll position. It was a good demo of scrubbing and a bad terminal: real
 * shells do not type their own output. You run a command and the output lands.
 * Only the command is ever typed, and that is a person typing, not the machine.
 * So nothing here animates text — a frame arrives with its command already run.
 *
 * What scroll still drives is which frame you are looking at, and that part is
 * genuinely scrubbed: the handoff is a continuous function of scroll position,
 * so it reverses if you scroll back rather than replaying.
 *
 * One step, one terminal, because when consecutive steps have different owners
 * a single shared shell says one actor ran all of them. A coding agent's work
 * does not happen in your sigiro shell. Separate frames make the ownership
 * structural instead of a caption.
 *
 * Design constraints, from interior's DESIGN.md:
 * - §5   the handoff uses the specified displacements — out at `y 6, blur(3px)`,
 *        in at `y 10, blur(6px)` — driven by scroll rather than by a duration.
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site.
 * - §3   no idle loops, so the resting cursor is a block that does not blink.
 * - §22  scroll-driven transforms are the accepted exception to the
 *        `useReducedMotion` rule. Reduced motion gets `fallback` instead, which
 *        is why `fallback` is required rather than optional.
 */

export type TerminalStep = {
  cmd: string;
  out: string;
  /** Names the step in the caption — "Diagnose", "Connect". */
  label: string;
  /** A chip beside the label: whose shell this is. */
  badge?: string;
  /** Ink rather than muted ink on the chip, for steps that are ours. */
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
 * The remainder is the handoff itself: frame out, then frame in.
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

function Screen({
  lines,
  height,
  style,
}: {
  lines: Line[];
  height: number;
  style?: React.CSSProperties;
}) {
  return (
    <div className="mat-well overflow-hidden rounded-[9px] px-3.5 py-3" style={{ height }}>
      <div
        aria-hidden
        className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
        style={style}
      >
        {lines.map((line, i) => (
          <div
            // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
            key={i}
            className={line.cmd ? 'text-ink' : 'text-ink-2'}
            style={{ height: LINE }}
          >
            {line.text}
          </div>
        ))}
        {/* The command finished and the shell is waiting. A block, not a caret:
            §3 bans idle loops, and a caret that blinks on a timer is one. */}
        <div className="text-ink" style={{ height: LINE }}>
          {'$ '}
          <span className="ml-px inline-block h-[13px] w-[7px] bg-ink align-[-1px]" />
        </div>
      </div>
    </div>
  );
}

export function SteppedTerminal({
  steps,
  fallback,
  track,
}: {
  steps: readonly TerminalStep[];
  /** Shown to reduced-motion and no-JS readers, who have nothing to scroll. */
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
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const frames = useMemo(() => plan(steps), [steps]);

  const { scrollYProgress } = useScroll({ target: ref, offset: ['start start', 'end end'] });
  const [progress, setProgress] = useState(0);
  useMotionValueEvent(scrollYProgress, 'change', (v) => {
    // Two decimals is finer than a pixel of travel and keeps renders bounded.
    const next = Math.round(v * 100) / 100;
    setProgress((prev) => (prev === next ? prev : next));
  });

  const scaled = Math.min(progress, 0.9999) * steps.length;
  const index = Math.floor(scaled);
  const frac = scaled - index;

  const handoff = frac <= HOLD || index >= steps.length - 1 ? 0 : (frac - HOLD) / (1 - HOLD);
  const leaving = handoff > 0 && handoff < 0.5;
  const arriving = handoff >= 0.5;
  const shown = arriving ? index + 1 : index;
  const step = steps[shown];

  let style: React.CSSProperties | undefined;
  if (leaving) {
    const t = handoff / 0.5;
    style = { opacity: 1 - t, transform: `translateY(${-6 * t}px)`, filter: `blur(${3 * t}px)` };
  } else if (arriving) {
    const t = (handoff - 0.5) / 0.5;
    style = {
      opacity: t,
      transform: `translateY(${10 * (1 - t)}px)`,
      filter: `blur(${6 * (1 - t)}px)`,
    };
  }

  /** Resizing is free across the handoff: the content it would jump is faded. */
  const height = handoff
    ? frames[index].height + (frames[index + 1].height - frames[index].height) * handoff
    : frames[shown].height;

  if (reduced) return <>{fallback}</>;

  return (
    <>
      <noscript>{fallback}</noscript>

      <div
        ref={ref}
        className="relative"
        style={{ height: track ?? `calc(100vh + ${steps.length * 45}vh)` }}
      >
        <div className="sticky top-0 flex h-screen items-center">
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

            <Screen lines={frames[shown].lines} height={height} style={style} />

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
